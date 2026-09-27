//! Route table + handlers. Every response is JSON except `/render/*` and
//! `/artifacts/*` (raw bytes); every error is the `{error, detail?}` envelope.

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderName, HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use dgm_scene::digest::{Digest, ObjectDigest, RigDigest};
use dgm_scene::ledger::{self, LedgerLine};
use dgm_scene::op::{Op, Outcome};
use glam::Vec3;
use serde::Deserialize;
use serde_json::{Map, Value, json};

use crate::SharedProject;
use crate::error::{ApiError, op_error, stage_error};
use crate::orchestrate::{self, DEFAULT_VIEW_PX, RenderKind};

pub fn router(state: SharedProject) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/scene", get(scene))
        .route("/pack", get(pack))
        .route("/ledger", get(ledger_lines))
        .route("/object/{name}", get(object))
        .route("/op", post(op_one))
        .route("/ops", post(op_batch))
        .route("/review", post(review))
        .route("/critique", post(critique))
        .route("/export", post(export))
        .route("/render/{kind}", get(render))
        .route("/artifacts/{file}", get(artifact))
        .route("/raycast", post(raycast))
        .layer(middleware::from_fn(localhost_only))
        .with_state(state)
}

// ---- host guard -----------------------------------------------------------

/// Reject anything whose `Host` is not this machine: the server only ever
/// binds 127.0.0.1, so a non-local Host means DNS rebinding, not a user.
async fn localhost_only(req: Request, next: Next) -> Response {
    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .map(str::to_owned)
        .or_else(|| req.uri().authority().map(|a| a.to_string()));
    if let Some(host) = host
        && !host_is_local(&host)
    {
        return ApiError::new(StatusCode::FORBIDDEN, "forbidden")
            .detail(format!("host `{host}` is not localhost"))
            .into_response();
    }
    next.run(req).await
}

fn host_is_local(host: &str) -> bool {
    let host = host.trim();
    let name = if let Some(rest) = host.strip_prefix('[') {
        rest.split(']').next().unwrap_or("")
    } else {
        match host.rsplit_once(':') {
            Some((h, port)) if !port.is_empty() && port.bytes().all(|b| b.is_ascii_digit()) => h,
            _ => host,
        }
    };
    matches!(name.to_ascii_lowercase().as_str(), "localhost" | "127.0.0.1" | "::1")
}

// ---- read side ------------------------------------------------------------

async fn health(State(state): State<SharedProject>) -> Json<Value> {
    let p = state.lock().await;
    Json(json!({
        "ok": true,
        "name": p.meta.name,
        "revision": p.doc.revision,
        "class": p.doc.asset_class,
    }))
}

async fn scene(State(state): State<SharedProject>) -> Json<Digest> {
    let p = state.lock().await;
    Json(orchestrate::scene_digest(&p))
}

async fn pack(State(state): State<SharedProject>) -> Json<Value> {
    let p = state.lock().await;
    let m = &p.pack.manifest;
    let rigs: Map<String, Value> =
        m.rigs.iter().map(|(k, v)| (k.clone(), json!({ "bones": v.bones.len() }))).collect();
    let clips: Map<String, Value> = m
        .clips
        .iter()
        .map(|(k, v)| {
            (
                k.clone(),
                json!({
                    "rig": v.rig,
                    "duration": v.duration,
                    "looped": v.looped,
                    "channels": v.channels.len(),
                }),
            )
        })
        .collect();
    Json(json!({
        "name": m.name,
        "version": m.version,
        "budgets": m.budgets,
        "texel_density": m.texel_density,
        "texture_sizes": m.texture_sizes,
        "palette": m.palette,
        "hard_edge_angle_deg": m.hard_edge_angle_deg,
        "uv_waste_max": m.uv_waste_max,
        "trims": m.trims,
        "rigs": rigs,
        "clips": clips,
        "reference_board": m.reference_board,
    }))
}

#[derive(Deserialize)]
struct LedgerQuery {
    #[serde(default)]
    from: u64,
}

async fn ledger_lines(
    State(state): State<SharedProject>,
    Query(q): Query<LedgerQuery>,
) -> Result<Json<Vec<LedgerLine>>, ApiError> {
    let p = state.lock().await;
    let lines = ledger::read(&p.ledger_path()).map_err(|e| {
        ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "ledger").detail(e.to_string())
    })?;
    Ok(Json(lines.into_iter().filter(|l| l.rev >= q.from).collect()))
}

async fn object(
    State(state): State<SharedProject>,
    Path(name): Path<String>,
) -> Result<Json<Value>, ApiError> {
    let p = state.lock().await;
    let o = p
        .doc
        .objects
        .get(&name)
        .ok_or_else(|| ApiError::not_found(format!("unknown object `{name}`")))?;
    let digest = ObjectDigest {
        verts: o.mesh.verts.len(),
        faces: o.mesh.faces.len(),
        tris: o.mesh.tri_count(),
        bounds: o.mesh.bounds().map(|(lo, hi)| [lo.to_array(), hi.to_array()]),
        material: o.material.clone(),
        seam_edges: o.mesh.seams.len(),
        rig: o.rig.as_ref().map(|r| RigDigest {
            preset: r.preset.clone(),
            bones: r.bones.len(),
            weighted_verts: r.weights.len(),
        }),
        lod_of: o.lod_of.clone(),
    };
    let islands = dgm_uv::islands(&o.mesh).len();
    let regions: Vec<String> = orchestrate::region_usage(&p.doc, &p.pack)
        .into_iter()
        .filter(|(_, objects)| objects.iter().any(|obj| obj == &name))
        .map(|(region, _)| region)
        .collect();
    Ok(Json(json!({ "object": digest, "islands": islands, "regions": regions })))
}

// ---- ops ------------------------------------------------------------------

fn parse_op(body: &str) -> Result<Op, ApiError> {
    serde_json::from_str(body).map_err(|e| ApiError::bad_request("bad op json", e.to_string()))
}

async fn op_one(
    State(state): State<SharedProject>,
    body: String,
) -> Result<Json<Outcome>, ApiError> {
    let op = parse_op(&body)?;
    let mut p = state.lock().await;
    let outcome = p.apply(op, "api").map_err(op_error)?;
    Ok(Json(outcome))
}

async fn op_batch(
    State(state): State<SharedProject>,
    body: String,
) -> Result<Json<Vec<Outcome>>, ApiError> {
    let raw: Vec<Value> = serde_json::from_str(&body)
        .map_err(|e| ApiError::bad_request("bad ops json", e.to_string()))?;
    let mut p = state.lock().await;
    let mut outcomes: Vec<Outcome> = Vec::with_capacity(raw.len());
    for (index, value) in raw.into_iter().enumerate() {
        let op: Op = serde_json::from_value(value).map_err(|e| {
            ApiError::bad_request("bad op json", e.to_string())
                .extra("index", json!(index))
                .extra("applied", json!(outcomes.len()))
        })?;
        let outcome = p.apply(op, "api").map_err(|e| {
            op_error(e).extra("index", json!(index)).extra("applied", json!(outcomes.len()))
        })?;
        outcomes.push(outcome);
    }
    Ok(Json(outcomes))
}

// ---- review / export ------------------------------------------------------

fn parse_or_default<T: Default + serde::de::DeserializeOwned>(
    body: &str,
    what: &str,
) -> Result<T, ApiError> {
    if body.trim().is_empty() {
        return Ok(T::default());
    }
    serde_json::from_str(body)
        .map_err(|e| ApiError::bad_request(&format!("bad {what} json"), e.to_string()))
}

async fn review(
    State(state): State<SharedProject>,
    body: String,
) -> Result<Json<dgm_jev::ReviewReport>, ApiError> {
    #[derive(Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ReviewBody {
        tier: Option<String>,
    }
    let rb: ReviewBody = parse_or_default(&body, "review")?;
    let tier = match rb.tier.as_deref() {
        None | Some("jev") => dgm_jev::Tier::Jev,
        Some("metrics") => dgm_jev::Tier::Metrics,
        Some("vision") => dgm_jev::Tier::Vision,
        Some(other) => {
            return Err(ApiError::bad_request(
                "bad tier",
                format!("unknown tier `{other}`; use metrics|jev|vision"),
            ));
        }
    };
    let p = state.lock().await;
    let report = orchestrate::review_project(&p, tier).await.map_err(stage_error)?;
    Ok(Json(report))
}

async fn critique(
    State(state): State<SharedProject>,
) -> Result<Json<dgm_jev::CritiqueReport>, ApiError> {
    let p = state.lock().await;
    let report = orchestrate::critique_project(&p).await.map_err(stage_error)?;
    Ok(Json(report))
}

async fn export(State(state): State<SharedProject>, body: String) -> Result<Json<Value>, ApiError> {
    #[derive(Default, Deserialize)]
    #[serde(deny_unknown_fields)]
    struct ExportBody {
        out: Option<String>,
    }
    let eb: ExportBody = parse_or_default(&body, "export")?;
    let p = state.lock().await;
    let out = orchestrate::export_project(&p, eb.out.as_deref()).map_err(stage_error)?;
    Ok(Json(json!({ "path": out.path, "bytes": out.bytes, "gate": out.gate })))
}

// ---- renders / artifacts / raycast ----------------------------------------

#[derive(Deserialize)]
struct RenderQuery {
    object: Option<String>,
    px: Option<u32>,
}

async fn render(
    State(state): State<SharedProject>,
    Path(kind): Path<String>,
    Query(q): Query<RenderQuery>,
) -> Result<Response, ApiError> {
    let kind = RenderKind::parse(&kind).ok_or_else(|| {
        ApiError::bad_request(
            "bad render kind",
            format!("unknown kind `{kind}`; use sheet|wireframe|uv|heatmap|filmstrip"),
        )
    })?;
    let p = state.lock().await;
    let out = orchestrate::render_project(&p, kind, q.object.as_deref(), q.px.unwrap_or(DEFAULT_VIEW_PX))
        .map_err(stage_error)?;
    let mut resp = (StatusCode::OK, out.png).into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
    resp.headers_mut().insert(
        HeaderName::from_static("x-artifact"),
        HeaderValue::from_str(&out.artifact).expect("artifact names are ascii"),
    );
    Ok(resp)
}

async fn artifact(
    State(state): State<SharedProject>,
    Path(file): Path<String>,
) -> Result<Response, ApiError> {
    if file.contains(['/', '\\']) || file.contains("..") {
        return Err(ApiError::bad_request("bad artifact name", format!("`{file}`")));
    }
    let path = {
        let p = state.lock().await;
        p.artifacts_dir().join(&file)
    };
    let bytes = std::fs::read(&path)
        .map_err(|_| ApiError::not_found(format!("no artifact `{file}`")))?;
    let content_type = match path.extension().and_then(|e| e.to_str()) {
        Some("png") => "image/png",
        Some("json") => "application/json",
        _ => "application/octet-stream",
    };
    let mut resp = (StatusCode::OK, bytes).into_response();
    resp.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    Ok(resp)
}

async fn raycast(
    State(state): State<SharedProject>,
    body: String,
) -> Result<Json<Value>, ApiError> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct RaycastBody {
        object: String,
        origin: [f32; 3],
        dir: [f32; 3],
    }
    let rb: RaycastBody = serde_json::from_str(&body)
        .map_err(|e| ApiError::bad_request("bad raycast json", e.to_string()))?;
    let p = state.lock().await;
    if !p.doc.objects.contains_key(&rb.object) {
        return Err(ApiError::not_found(format!("unknown object `{}`", rb.object)));
    }
    let hit = dgm_render::raycast(&p.doc, &rb.object, Vec3::from(rb.origin), Vec3::from(rb.dir));
    Ok(Json(match hit {
        Some(h) => json!({
            "face": h.face.to_string(),
            "distance": h.distance,
            "point": [h.point.x, h.point.y, h.point.z],
        }),
        None => Value::Null,
    }))
}

#[cfg(test)]
mod tests {
    use super::host_is_local;

    #[test]
    fn host_guard_accepts_local_forms_only() {
        for ok in ["localhost", "localhost:7799", "127.0.0.1", "127.0.0.1:80", "[::1]:7799", "LOCALHOST"] {
            assert!(host_is_local(ok), "{ok} should be local");
        }
        for bad in ["evil.example", "evil.example:7799", "127.0.0.1.evil.example", "192.168.1.4:7799", ""] {
            assert!(!host_is_local(bad), "{bad} should be rejected");
        }
    }
}
