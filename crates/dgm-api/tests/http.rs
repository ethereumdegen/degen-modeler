//! End-to-end HTTP flow against a server on an ephemeral port: ops mutate the
//! shared project, digests reflect it, and the error envelope carries the
//! contract statuses.

use std::path::Path;
use std::sync::Arc;

use dgm_atlas::AssetClass;
use dgm_scene::project::Project;
use serde_json::Value;
use tokio::sync::Mutex;

async fn spawn_app() -> (String, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let pack_src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/classic");
    let project = Project::init(&dir.path().join("proj"), &pack_src, "itest", AssetClass::Prop, None)
        .expect("project init");
    let state = Arc::new(Mutex::new(project));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, dgm_api::router(state)).await.expect("serve");
    });
    (format!("http://{addr}"), dir)
}

#[tokio::test]
async fn op_flow_digest_and_errors() {
    let (base, _dir) = spawn_app().await;
    let client = reqwest::Client::new();

    // Health: fresh project at revision 0.
    let health: Value =
        client.get(format!("{base}/health")).send().await.unwrap().json().await.unwrap();
    assert_eq!(health["ok"], true);
    assert_eq!(health["name"], "itest");
    assert_eq!(health["revision"], 0);
    assert_eq!(health["class"], "prop");

    // A prim_box op lands revision 1.
    let res = client
        .post(format!("{base}/op"))
        .body(r#"{"op":"prim_box","object":"crate","size":[1.0,1.0,1.0]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let outcome: Value = res.json().await.unwrap();
    assert_eq!(outcome["revision"], 1);

    // The scene digest shows the object with box topology.
    let scene: Value =
        client.get(format!("{base}/scene")).send().await.unwrap().json().await.unwrap();
    assert_eq!(scene["revision"], 1);
    assert_eq!(scene["objects"]["crate"]["verts"], 8);
    assert_eq!(scene["objects"]["crate"]["faces"], 6);
    assert_eq!(scene["objects"]["crate"]["tris"], 12);
    assert_eq!(scene["budget"]["used_tris"], 12);

    // A budget-breaking op fails closed: 409, findings attached, no revision.
    let res = client
        .post(format!("{base}/op"))
        .body(r#"{"op":"prim_cylinder","object":"huge","radius":1.0,"height":1.0,"segments":600}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 409);
    let err: Value = res.json().await.unwrap();
    assert_eq!(err["error"], "budget exceeded");
    assert_eq!(err["findings"][0]["rule"], "budget.tris");

    // Bad op JSON: 400 and the detail names real ops so agents can self-fix.
    let res = client
        .post(format!("{base}/op"))
        .body(r#"{"op":"prim_bax","object":"x"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 400);
    let err: Value = res.json().await.unwrap();
    assert_eq!(err["error"], "bad op json");
    let detail = err["detail"].as_str().unwrap();
    assert!(detail.contains("unknown variant `prim_bax`"), "detail: {detail}");
    assert!(detail.contains("prim_box"), "detail should hint real op names: {detail}");

    // Budget failure and parse failure never touched the ledger.
    let ledger: Value =
        client.get(format!("{base}/ledger?from=0")).send().await.unwrap().json().await.unwrap();
    assert_eq!(ledger.as_array().unwrap().len(), 1);
    assert_eq!(ledger[0]["rev"], 1);
    assert_eq!(ledger[0]["actor"], "api");

    // /ops stops at the first failure and reports its index.
    let res = client
        .post(format!("{base}/ops"))
        .body(
            r#"[{"op":"prim_box","object":"b2","size":[0.5,0.5,0.5]},
                {"op":"object_material","object":"b2","material":"nope"}]"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 404);
    let err: Value = res.json().await.unwrap();
    assert_eq!(err["index"], 1);
    assert_eq!(err["applied"], 1);

    // Unknown object: 404 envelope.
    let res = client.get(format!("{base}/object/ghost")).send().await.unwrap();
    assert_eq!(res.status(), 404);

    // /object/{name} carries the digest plus island count.
    let res = client.get(format!("{base}/object/crate")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    let obj: Value = res.json().await.unwrap();
    assert_eq!(obj["object"]["tris"], 12);
    assert!(obj["islands"].is_u64());

    // Pack summary exposes budgets and trims.
    let pack: Value =
        client.get(format!("{base}/pack")).send().await.unwrap().json().await.unwrap();
    assert_eq!(pack["name"], "classic");
    assert_eq!(pack["budgets"]["prop"]["max_tris"], 800);
    assert!(pack["trims"]["wood"]["regions"]["planks"].is_object());

    // Foreign Host header: refused even though the TCP peer is local.
    let res = client
        .get(format!("{base}/health"))
        .header("host", "evil.example:7799")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 403);
}

#[tokio::test]
async fn interior_render_and_digest_extras() {
    let (base, _dir) = spawn_app().await;
    let client = reqwest::Client::new();

    let res = client
        .post(format!("{base}/ops"))
        .body(
            r#"[{"op":"prim_box","object":"room","size":[4.0,3.0,4.0]},
                {"op":"set_reference","path":"refs/cave.jpg"},
                {"op":"tag_object","object":"room","tag":"open"}]"#,
        )
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);

    // The digest surfaces references and tags under `extra`.
    let scene: Value =
        client.get(format!("{base}/scene")).send().await.unwrap().json().await.unwrap();
    assert_eq!(scene["extra"]["references"], serde_json::json!(["refs/cave.jpg"]));
    assert_eq!(scene["extra"]["tags"]["room"], serde_json::json!(["open"]));
    assert!(scene["extra"]["metrics"]["connectivity"].is_number());

    // /render/interior: a 2x2 sheet of `px` tiles, named like the others.
    let res = client.get(format!("{base}/render/interior?px=64")).send().await.unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.headers()["content-type"], "image/png");
    assert_eq!(res.headers()["x-artifact"], "r3-interior.png");
    let png = res.bytes().await.unwrap();
    let img = image::load_from_memory(&png).unwrap();
    assert_eq!((img.width(), img.height()), (128, 128));

    let res = client.get(format!("{base}/render/nope")).send().await.unwrap();
    assert_eq!(res.status(), 400);
    let err: Value = res.json().await.unwrap();
    assert!(err["detail"].as_str().unwrap().contains("interior"));
}
