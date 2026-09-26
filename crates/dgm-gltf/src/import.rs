//! Round-trip check: parse a glTF/GLB with the `gltf` crate and count what
//! landed.

use serde_json::{Value, json};

use crate::GltfError;

/// Summarize exported bytes: `{nodes, meshes, prims, tris, materials,
/// textures, skins, animations, unlit}`. `tris` counts triangle-mode
/// primitives by index count / 3; `unlit` is true when any material carries
/// `KHR_materials_unlit`.
pub fn import_summary(bytes: &[u8]) -> Result<Value, GltfError> {
    let gltf = gltf::Gltf::from_slice(bytes)?;
    let doc = &gltf.document;
    let mut prims = 0usize;
    let mut tris = 0usize;
    for mesh in doc.meshes() {
        for prim in mesh.primitives() {
            prims += 1;
            if prim.mode() == gltf::mesh::Mode::Triangles {
                let n = prim
                    .indices()
                    .or_else(|| prim.get(&gltf::Semantic::Positions))
                    .map_or(0, |a| a.count());
                tris += n / 3;
            }
        }
    }
    Ok(json!({
        "nodes": doc.nodes().count(),
        "meshes": doc.meshes().count(),
        "prims": prims,
        "tris": tris,
        "materials": doc.materials().count(),
        "textures": doc.textures().count(),
        "skins": doc.skins().count(),
        "animations": doc.animations().count(),
        "unlit": doc.materials().any(|m| m.unlit()),
    }))
}
