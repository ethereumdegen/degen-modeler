//! Deterministic CPU measurement renderer for Degen Modeler.
//!
//! Pure `f32` math over the scene document: perspective contact sheets,
//! wireframes, UV layouts, texel-density heatmaps, turntable filmstrips,
//! silhouette masks, raycasts and the numeric [`Metrics`] block. No GPU, no
//! threads, no wall clock — identical inputs give identical pixels.

mod camera;
mod env_metrics;
mod geom;
mod interior;
mod metrics;
mod raster;
mod raycast;
mod views;

pub use interior::interior_sheet;
pub use metrics::{Metrics, ObjectMetrics, metrics};
pub use raycast::{RayHit, raycast};
pub use views::{
    contact_sheet, filmstrip, heatmap, silhouette_masks, uv_layout, wireframe_sheet,
};

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("unknown object \"{0}\"")]
    UnknownObject(String),
    #[error("{0}")]
    Invalid(String),
}

pub(crate) fn check_px(px: u32) -> Result<(), RenderError> {
    if px == 0 {
        return Err(RenderError::Invalid("view size must be > 0 px".into()));
    }
    Ok(())
}
