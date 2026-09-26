use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable vertex id; unique for the life of a mesh, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VertId(pub u32);

/// Stable face id; unique for the life of a mesh, never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FaceId(pub u32);

impl fmt::Display for VertId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "v{}", self.0)
    }
}

impl fmt::Display for FaceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "f{}", self.0)
    }
}

/// Undirected edge key, normalized so the lower vertex id comes first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct EdgeKey(pub VertId, pub VertId);

impl EdgeKey {
    pub fn new(a: VertId, b: VertId) -> Self {
        if a <= b { Self(a, b) } else { Self(b, a) }
    }
}

impl fmt::Display for EdgeKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}:{}", self.0.0, self.1.0)
    }
}
