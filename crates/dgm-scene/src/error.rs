use dgm_mesh::{Finding, MeshError};

#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("unknown object `{0}`")]
    UnknownObject(String),
    #[error("object `{0}` already exists")]
    ObjectExists(String),
    #[error("unknown selection `{0}`")]
    UnknownSelection(String),
    #[error("unknown material `{0}`")]
    UnknownMaterial(String),
    #[error("unknown clip `{0}`")]
    UnknownClip(String),
    #[error("unknown bone `{0}`")]
    UnknownBone(String),
    #[error("selection kind mismatch: {0}")]
    SelectionKind(String),
    #[error("bad parameters: {0}")]
    BadParams(String),
    #[error("budget exceeded: {message}")]
    Budget { message: String, findings: Vec<Finding> },
    #[error("pack: {0}")]
    Pack(String),
    #[error(transparent)]
    Mesh(#[from] MeshError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// Integration placeholder: removed once every family is routed.
    #[error("op `{0}` is not routed yet")]
    Unrouted(&'static str),
}
