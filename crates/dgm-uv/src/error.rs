use dgm_mesh::{FaceId, MeshError};

/// UV toolkit errors; messages carry display ids ("f12", "e4:9").
#[derive(Debug, thiserror::Error)]
pub enum UvError {
    #[error("unknown face {0}")]
    UnknownFace(FaceId),
    #[error("empty face selection")]
    EmptySelection,
    #[error("unknown trim region {sheet}/{region}")]
    UnknownRegion { sheet: String, region: String },
    #[error(transparent)]
    Mesh(#[from] MeshError),
    #[error("{0}")]
    Invalid(String),
}
