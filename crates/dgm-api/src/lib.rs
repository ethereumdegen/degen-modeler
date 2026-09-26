//! Agent API: axum over one shared [`Project`], strictly localhost.
//!
//! The server binds `127.0.0.1:<port>` and additionally rejects any request
//! whose `Host` header is not localhost, so a browser on the same machine
//! cannot be tricked into driving it cross-origin from a public site.

pub mod error;
pub mod orchestrate;
pub mod routes;

use std::sync::Arc;

use dgm_scene::project::Project;
use tokio::sync::Mutex;

pub use error::ApiError;
pub use routes::router;

/// One project, shared by every handler. Ops serialize through the lock, so
/// ledger revisions stay strictly ordered.
pub type SharedProject = Arc<Mutex<Project>>;

pub const DEFAULT_PORT: u16 = 7799;

/// Bind `127.0.0.1:<port>` and serve until the task is dropped.
pub async fn serve(project: Project, port: u16) -> std::io::Result<()> {
    let state: SharedProject = Arc::new(Mutex::new(project));
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await?;
    axum::serve(listener, router(state)).await
}
