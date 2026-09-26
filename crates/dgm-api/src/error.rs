//! The JSON error envelope: `{error, detail?, ...extras}` with contract
//! statuses (400 bad op JSON, 404 unknown, 409 budget/gate, 502 upstream).

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

use crate::orchestrate::StageError;

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub error: String,
    pub detail: Option<String>,
    pub extra: Map<String, Value>,
}

impl ApiError {
    pub fn new(status: StatusCode, error: impl Into<String>) -> Self {
        Self { status, error: error.into(), detail: None, extra: Map::new() }
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn extra(mut self, key: &str, value: Value) -> Self {
        self.extra.insert(key.into(), value);
        self
    }

    pub fn bad_request(error: &str, detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, error).detail(detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "unknown").detail(detail)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let mut body = Map::new();
        body.insert("error".into(), json!(self.error));
        if let Some(detail) = self.detail {
            body.insert("detail".into(), json!(detail));
        }
        body.extend(self.extra);
        (self.status, Json(Value::Object(body))).into_response()
    }
}

/// Op failures onto the wire: 404 unknown element, 409 budget/conflict,
/// 400 bad parameters, 500 anything the server broke on its own.
pub fn op_error(e: dgm_scene::OpError) -> ApiError {
    use dgm_scene::OpError as E;
    let msg = e.to_string();
    match e {
        E::UnknownObject(_)
        | E::UnknownSelection(_)
        | E::UnknownMaterial(_)
        | E::UnknownClip(_)
        | E::UnknownBone(_) => ApiError::not_found(msg),
        E::Budget { findings, .. } => ApiError::new(StatusCode::CONFLICT, "budget exceeded")
            .detail(msg)
            .extra("findings", serde_json::to_value(&findings).unwrap_or(Value::Null)),
        E::ObjectExists(_) => ApiError::new(StatusCode::CONFLICT, "conflict").detail(msg),
        E::BadParams(_) | E::SelectionKind(_) | E::Pack(_) | E::Mesh(_) | E::Uv(_) => {
            ApiError::bad_request("bad op", msg)
        }
        E::Io(_) => {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal").detail(msg)
        }
    }
}

/// Orchestration failures onto the wire; 502 is reserved for the upstream
/// review call, 409 for the export gate.
pub fn stage_error(e: StageError) -> ApiError {
    let msg = e.to_string();
    match e {
        StageError::BadRequest(_) => ApiError::bad_request("bad request", msg),
        StageError::UnknownObject(_) => ApiError::not_found(msg),
        StageError::GateFailed(report) => ApiError::new(StatusCode::CONFLICT, "gate failed")
            .extra("report", serde_json::to_value(&report).unwrap_or(Value::Null)),
        StageError::Render(_) | StageError::Gltf(_) | StageError::Io(..) => {
            ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal").detail(msg)
        }
        StageError::Review(_) => ApiError::new(StatusCode::BAD_GATEWAY, "review upstream").detail(msg),
    }
}
