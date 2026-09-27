//! Hand-rolled deterministic GLB writer.
//!
//! Determinism rules:
//! - JSON object keys are inserted in one fixed order everywhere;
//!   `serde_json`'s `preserve_order` feature keeps that order on write.
//! - One BIN buffer, appended in a fixed phase order: interned texture PNGs
//!   (material order), then per-object vertex/index data (object order),
//!   then inverse bind matrices, then animation curves (clip order). Every
//!   buffer view starts on a 4-byte boundary.
//! - No timestamps, no hashing, no `HashMap` iteration, no platform paths
//!   in the output.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use dgm_atlas::Pack;
use dgm_mesh::{FaceId, Mesh, VertId};
use dgm_scene::{AlphaMode, Doc, Material, Rig, TextureRef};
use glam::{Mat4, Quat, Vec3};
use serde_json::{Map, Value, json};

use crate::GltfError;

const UNSIGNED_BYTE: u64 = 5121;
const UNSIGNED_SHORT: u64 = 5123;
const UNSIGNED_INT: u64 = 5125;
const FLOAT: u64 = 5126;
const ARRAY_BUFFER: u64 = 34962;
const ELEMENT_ARRAY_BUFFER: u64 = 34963;
const NEAREST: u64 = 9728;
const REPEAT: u64 = 10497;

/// Options for [`export_glb`].
#[derive(Debug, Default, Clone)]
pub struct ExportOptions {
    /// Base directory `TextureRef::File` paths resolve against (the project
    /// dir). Trim sheets resolve through the pack instead.
    pub base_dir: PathBuf,
    /// Gate/review report embedded verbatim as the glTF root `extras`.
    pub embed_report: Option<Value>,
}

/// Export the doc as a binary glTF (.glb) byte vector.
///
/// - Per object: one mesh + one node named after the object. Corners with
///   identical (position, corner normal at the pack's hard-edge angle, UV)
///   weld into one glTF vertex; on rigged objects the skin influence joins
///   the weld key so distinct weights never collapse.
/// - Materials: `pbrMetallicRoughness` base colour (texture for Trim/File,
///   factor for Color) plus the `KHR_materials_unlit` extension as the
///   primary look; `alphaMode: MASK` with cutoff 0.5; `doubleSided` as set.
/// - Rigs: joints are a node subtree under the object node. A bone node's
///   translation is `head - parent.head`; inverse bind matrices are
///   `translate(-head)` (world rest is a pure translation to the head).
/// - Clips: one animation per clip, LINEAR samplers. Rotation keys are
///   absolute quaternions; **translation keys in `dgm_scene::Channel` are
///   relative deltas** — the exporter adds them to the bone's rest
///   translation so engines receive absolute node translations. Scales
///   pass through unchanged.
pub fn export_glb(doc: &Doc, pack: &Pack, opts: &ExportOptions) -> Result<Vec<u8>, GltfError> {
    let mut b = Builder::default();

    // Phase 1: materials, interning textures (PNG bytes lead the BIN).
    let mut tex = TexInterner::default();
    let mut material_index: BTreeMap<&str, usize> = BTreeMap::new();
    let mut materials: Vec<Value> = Vec::new();
    for (name, mat) in &doc.materials {
        material_index.insert(name, materials.len());
        materials.push(material_json(name, mat, pack, &opts.base_dir, &mut b, &mut tex)?);
    }

    // Phase 2: nodes + meshes + skins, object order. Bones become child
    // nodes right after their object's node.
    let mut nodes: Vec<Value> = Vec::new();
    let mut meshes: Vec<Value> = Vec::new();
    let mut skins: Vec<Value> = Vec::new();
    let mut scene_nodes: Vec<u64> = Vec::new();
    // (object, bone) -> (node index, rest translation) for animations.
    let mut bone_nodes: BTreeMap<(&str, &str), (usize, Vec3)> = BTreeMap::new();

    for (name, obj) in &doc.objects {
        let obj_node = nodes.len();
        scene_nodes.push(obj_node as u64);
        // Rigs with zero bones can't form a glTF skin; treat as unrigged.
        let rig = obj.rig.as_ref().filter(|r| !r.bones.is_empty());

        let mut node = Map::new();
        node.insert("name".into(), json!(name));
        if let Some(rig) = rig {
            let roots: Vec<u64> = rig
                .bones
                .iter()
                .enumerate()
                .filter(|(_, bone)| bone.parent.is_none())
                .map(|(i, _)| (obj_node + 1 + i) as u64)
                .collect();
            if !roots.is_empty() {
                node.insert("children".into(), json!(roots));
            }
        }
        if !obj.mesh.faces.is_empty() {
            node.insert("mesh".into(), json!(meshes.len()));
            if rig.is_some() {
                node.insert("skin".into(), json!(skins.len()));
            }
        }
        nodes.push(Value::Object(node));

        if let Some(rig) = rig {
            for (i, bone) in rig.bones.iter().enumerate() {
                let parent_head = match bone.parent {
                    None => Vec3::ZERO,
                    Some(p) => {
                        rig.bones
                            .get(p as usize)
                            .ok_or_else(|| GltfError::BadBoneParent {
                                object: name.clone(),
                                bone: bone.name.clone(),
                                parent: p,
                            })?
                            .head
                    }
                };
                let rest = bone.head - parent_head;
                let mut bn = Map::new();
                bn.insert("name".into(), json!(bone.name));
                let children: Vec<u64> = rig
                    .bones
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.parent == Some(i as u16))
                    .map(|(j, _)| (obj_node + 1 + j) as u64)
                    .collect();
                if !children.is_empty() {
                    bn.insert("children".into(), json!(children));
                }
                bn.insert("translation".into(), json!([rest.x, rest.y, rest.z]));
                nodes.push(Value::Object(bn));
                bone_nodes.insert((name.as_str(), bone.name.as_str()), (obj_node + 1 + i, rest));
            }
        }

        if obj.mesh.faces.is_empty() {
            continue;
        }
        let prim = weld(&obj.mesh, pack.manifest.hard_edge_angle_deg, rig);
        let mut attrs = Map::new();

        let pos_bytes: Vec<u8> =
            prim.positions.iter().flat_map(|p| p.to_array()).flat_map(f32::to_le_bytes).collect();
        let view = b.view(&pos_bytes, Some(ARRAY_BUFFER));
        let (mut lo, mut hi) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
        for p in &prim.positions {
            lo = lo.min(*p);
            hi = hi.max(*p);
        }
        let minmax = Some((json!([lo.x, lo.y, lo.z]), json!([hi.x, hi.y, hi.z])));
        attrs.insert(
            "POSITION".into(),
            json!(b.accessor(view, FLOAT, prim.positions.len(), "VEC3", minmax)),
        );

        let nrm_bytes: Vec<u8> =
            prim.normals.iter().flat_map(|n| n.to_array()).flat_map(f32::to_le_bytes).collect();
        let view = b.view(&nrm_bytes, Some(ARRAY_BUFFER));
        attrs.insert(
            "NORMAL".into(),
            json!(b.accessor(view, FLOAT, prim.normals.len(), "VEC3", None)),
        );

        let uv_bytes: Vec<u8> =
            prim.uvs.iter().flatten().copied().flat_map(f32::to_le_bytes).collect();
        let view = b.view(&uv_bytes, Some(ARRAY_BUFFER));
        attrs.insert(
            "TEXCOORD_0".into(),
            json!(b.accessor(view, FLOAT, prim.uvs.len(), "VEC2", None)),
        );

        // Baked lighting / masks: u8-normalized COLOR_0, only when the mesh
        // carries any vertex colors (absent verts read as white). VEC4 with
        // alpha 255 so every element is 4-byte aligned, as glTF requires.
        if !obj.mesh.colors.is_empty() {
            let col_bytes: Vec<u8> = prim
                .colors
                .iter()
                .flat_map(|c| {
                    let [r, g, b] = c.map(|x| (x.clamp(0.0, 1.0) * 255.0).round() as u8);
                    [r, g, b, 255]
                })
                .collect();
            let view = b.view(&col_bytes, Some(ARRAY_BUFFER));
            let acc = b.accessor(view, UNSIGNED_BYTE, prim.colors.len(), "VEC4", None);
            if let Some(a) = b.accessors[acc].as_object_mut() {
                a.insert("normalized".into(), json!(true));
            }
            attrs.insert("COLOR_0".into(), json!(acc));
        }

        if rig.is_some() {
            let joint_bytes: Vec<u8> = prim.joints.iter().flatten().copied().collect();
            let view = b.view(&joint_bytes, Some(ARRAY_BUFFER));
            attrs.insert(
                "JOINTS_0".into(),
                json!(b.accessor(view, UNSIGNED_BYTE, prim.joints.len(), "VEC4", None)),
            );
            let weight_bytes: Vec<u8> =
                prim.weights.iter().flatten().copied().flat_map(f32::to_le_bytes).collect();
            let view = b.view(&weight_bytes, Some(ARRAY_BUFFER));
            attrs.insert(
                "WEIGHTS_0".into(),
                json!(b.accessor(view, FLOAT, prim.weights.len(), "VEC4", None)),
            );
        }

        // u16 indices whenever every index fits, else u32.
        let indices = if prim.positions.len() <= u16::MAX as usize + 1 {
            let bytes: Vec<u8> =
                prim.indices.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect();
            let view = b.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
            b.accessor(view, UNSIGNED_SHORT, prim.indices.len(), "SCALAR", None)
        } else {
            let bytes: Vec<u8> = prim.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
            let view = b.view(&bytes, Some(ELEMENT_ARRAY_BUFFER));
            b.accessor(view, UNSIGNED_INT, prim.indices.len(), "SCALAR", None)
        };

        let mut p = Map::new();
        p.insert("attributes".into(), Value::Object(attrs));
        p.insert("indices".into(), json!(indices));
        if let Some(mat_name) = &obj.material {
            let mi = material_index.get(mat_name.as_str()).ok_or_else(|| {
                GltfError::UnknownMaterial { object: name.clone(), material: mat_name.clone() }
            })?;
            p.insert("material".into(), json!(mi));
        }
        meshes.push(json!({ "name": name, "primitives": [Value::Object(p)] }));

        if let Some(rig) = rig {
            let ibm_bytes: Vec<u8> = rig
                .bones
                .iter()
                .flat_map(|bone| Mat4::from_translation(-bone.head).to_cols_array())
                .flat_map(f32::to_le_bytes)
                .collect();
            let view = b.view(&ibm_bytes, None);
            let ibm = b.accessor(view, FLOAT, rig.bones.len(), "MAT4", None);
            let joints: Vec<u64> = (0..rig.bones.len()).map(|i| (obj_node + 1 + i) as u64).collect();
            skins.push(json!({ "name": name, "inverseBindMatrices": ibm, "joints": joints }));
        }
    }

    // Phase 3: animations, clip order; per bone the curve order is fixed:
    // rotation, translation, scale, sharing one input accessor.
    let mut animations: Vec<Value> = Vec::new();
    for (clip_name, clip) in &doc.clips {
        let mut samplers: Vec<Value> = Vec::new();
        let mut channels: Vec<Value> = Vec::new();
        for (bone, ch) in &clip.channels {
            if ch.times.is_empty() {
                continue;
            }
            let &(node, rest) =
                bone_nodes.get(&(clip.object.as_str(), bone.as_str())).ok_or_else(|| {
                    GltfError::ClipBone {
                        clip: clip_name.clone(),
                        object: clip.object.clone(),
                        bone: bone.clone(),
                    }
                })?;
            let curve_len = |kind, got: usize| {
                if got == ch.times.len() {
                    Ok(())
                } else {
                    Err(GltfError::CurveLength {
                        clip: clip_name.clone(),
                        bone: bone.clone(),
                        kind,
                        got,
                        want: ch.times.len(),
                    })
                }
            };

            let time_bytes: Vec<u8> = ch.times.iter().flat_map(|t| t.to_le_bytes()).collect();
            let view = b.view(&time_bytes, None);
            let (lo, hi) = ch
                .times
                .iter()
                .fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), &t| (lo.min(t), hi.max(t)));
            let input =
                b.accessor(view, FLOAT, ch.times.len(), "SCALAR", Some((json!([lo]), json!([hi]))));

            let mut curve = |b: &mut Builder, data: &[u8], count, ty, path| {
                let view = b.view(data, None);
                let output = b.accessor(view, FLOAT, count, ty, None);
                samplers.push(
                    json!({ "input": input, "interpolation": "LINEAR", "output": output }),
                );
                channels.push(json!({
                    "sampler": samplers.len() - 1,
                    "target": { "node": node, "path": path },
                }));
            };

            if let Some(rots) = &ch.rotations {
                curve_len("rotations", rots.len())?;
                let bytes: Vec<u8> = rots
                    .iter()
                    .flat_map(|r| {
                        let q = Quat::from_array(*r);
                        let q = if q.length_squared() > 1e-12 { q.normalize() } else { Quat::IDENTITY };
                        q.to_array()
                    })
                    .flat_map(f32::to_le_bytes)
                    .collect();
                curve(&mut b, &bytes, rots.len(), "VEC4", "rotation");
            }
            if let Some(trs) = &ch.translations {
                curve_len("translations", trs.len())?;
                // Relative deltas -> absolute node translations.
                let bytes: Vec<u8> = trs
                    .iter()
                    .flat_map(|t| (rest + Vec3::from(*t)).to_array())
                    .flat_map(f32::to_le_bytes)
                    .collect();
                curve(&mut b, &bytes, trs.len(), "VEC3", "translation");
            }
            if let Some(scs) = &ch.scales {
                curve_len("scales", scs.len())?;
                let bytes: Vec<u8> =
                    scs.iter().flatten().copied().flat_map(f32::to_le_bytes).collect();
                curve(&mut b, &bytes, scs.len(), "VEC3", "scale");
            }
        }
        if !channels.is_empty() {
            animations.push(json!({ "name": clip_name, "channels": channels, "samplers": samplers }));
        }
    }

    // Root JSON, fixed key order. glTF forbids empty arrays, so optional
    // sections appear only when populated.
    let mut root = Map::new();
    root.insert("asset".into(), json!({ "version": "2.0", "generator": "degen-modeler" }));
    if !materials.is_empty() {
        let mut used = Vec::new();
        if doc.materials.values().any(|m| m.emissive.is_none()) {
            used.push("KHR_materials_unlit");
        }
        if doc.materials.values().any(|m| {
            m.emissive.is_some() && (m.emissive_strength - 1.0).abs() > f32::EPSILON
        }) {
            used.push("KHR_materials_emissive_strength");
        }
        if !used.is_empty() {
            root.insert("extensionsUsed".into(), json!(used));
        }
    }
    if !nodes.is_empty() {
        root.insert("scene".into(), json!(0));
        root.insert("scenes".into(), json!([{ "nodes": scene_nodes }]));
        root.insert("nodes".into(), Value::Array(nodes));
    }
    if !meshes.is_empty() {
        root.insert("meshes".into(), Value::Array(meshes));
    }
    if !materials.is_empty() {
        root.insert("materials".into(), Value::Array(materials));
    }
    if !tex.textures.is_empty() {
        root.insert("textures".into(), Value::Array(tex.textures));
        root.insert("images".into(), Value::Array(tex.images));
        root.insert(
            "samplers".into(),
            json!([{ "magFilter": NEAREST, "minFilter": NEAREST, "wrapS": REPEAT, "wrapT": REPEAT }]),
        );
    }
    if !skins.is_empty() {
        root.insert("skins".into(), Value::Array(skins));
    }
    if !animations.is_empty() {
        root.insert("animations".into(), Value::Array(animations));
    }
    if !b.accessors.is_empty() {
        root.insert("accessors".into(), Value::Array(b.accessors));
        root.insert("bufferViews".into(), Value::Array(b.views));
        root.insert("buffers".into(), json!([{ "byteLength": b.bin.len() }]));
    }
    if let Some(report) = &opts.embed_report {
        root.insert("extras".into(), report.clone());
    }

    Ok(pack_glb(serde_json::to_vec(&Value::Object(root))?, b.bin))
}

/// BIN buffer plus the bufferView/accessor JSON that indexes into it.
#[derive(Default)]
struct Builder {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Builder {
    fn view(&mut self, bytes: &[u8], target: Option<u64>) -> usize {
        while self.bin.len() % 4 != 0 {
            self.bin.push(0);
        }
        let mut v = Map::new();
        v.insert("buffer".into(), json!(0));
        v.insert("byteOffset".into(), json!(self.bin.len()));
        v.insert("byteLength".into(), json!(bytes.len()));
        if let Some(t) = target {
            v.insert("target".into(), json!(t));
        }
        self.bin.extend_from_slice(bytes);
        self.views.push(Value::Object(v));
        self.views.len() - 1
    }

    fn accessor(
        &mut self,
        view: usize,
        component_type: u64,
        count: usize,
        ty: &str,
        minmax: Option<(Value, Value)>,
    ) -> usize {
        let mut a = Map::new();
        a.insert("bufferView".into(), json!(view));
        a.insert("componentType".into(), json!(component_type));
        a.insert("count".into(), json!(count));
        a.insert("type".into(), json!(ty));
        if let Some((min, max)) = minmax {
            a.insert("min".into(), min);
            a.insert("max".into(), max);
        }
        self.accessors.push(Value::Object(a));
        self.accessors.len() - 1
    }
}

/// Texture/image lists with dedup: the same trim sheet or file embeds once.
/// Indices assigned at first use while walking materials in name order.
#[derive(Default)]
struct TexInterner {
    index: BTreeMap<String, usize>,
    images: Vec<Value>,
    textures: Vec<Value>,
}

impl TexInterner {
    fn intern(
        &mut self,
        b: &mut Builder,
        key: String,
        path: &Path,
        name: &str,
    ) -> Result<usize, GltfError> {
        if let Some(&i) = self.index.get(&key) {
            return Ok(i);
        }
        let bytes = std::fs::read(path)
            .map_err(|source| GltfError::Texture { path: path.to_path_buf(), source })?;
        let view = b.view(&bytes, None);
        self.images.push(json!({ "name": name, "mimeType": "image/png", "bufferView": view }));
        self.textures.push(json!({ "sampler": 0, "source": self.images.len() - 1 }));
        let idx = self.textures.len() - 1;
        self.index.insert(key, idx);
        Ok(idx)
    }
}

fn material_json(
    name: &str,
    mat: &Material,
    pack: &Pack,
    base_dir: &Path,
    b: &mut Builder,
    tex: &mut TexInterner,
) -> Result<Value, GltfError> {
    let mut pbr = Map::new();
    match &mat.texture {
        TextureRef::Trim { sheet } => {
            let path = pack.texture_path(sheet).ok_or_else(|| GltfError::UnknownSheet {
                material: name.to_owned(),
                sheet: sheet.clone(),
            })?;
            let ti = tex.intern(b, format!("trim:{sheet}"), &path, sheet)?;
            pbr.insert("baseColorTexture".into(), json!({ "index": ti }));
            // Lit fallback: factor stays white so the texture shows as-is.
            pbr.insert("baseColorFactor".into(), json!([1.0, 1.0, 1.0, 1.0]));
        }
        TextureRef::File { path } => {
            let full = base_dir.join(path);
            let ti = tex.intern(b, format!("file:{path}"), &full, path)?;
            pbr.insert("baseColorTexture".into(), json!({ "index": ti }));
            pbr.insert("baseColorFactor".into(), json!([1.0, 1.0, 1.0, 1.0]));
        }
        TextureRef::Color { rgba } => {
            pbr.insert("baseColorFactor".into(), json!(rgba.map(|c| c as f32 / 255.0)));
        }
    }
    pbr.insert("metallicFactor".into(), json!(0.0));
    pbr.insert("roughnessFactor".into(), json!(1.0));

    let mut m = Map::new();
    m.insert("name".into(), json!(name));
    m.insert("pbrMetallicRoughness".into(), Value::Object(pbr));
    if mat.alpha == AlphaMode::Mask {
        m.insert("alphaMode".into(), json!("MASK"));
        m.insert("alphaCutoff".into(), json!(0.5));
    }
    if mat.double_sided {
        m.insert("doubleSided".into(), json!(true));
    }
    let mut ext = Map::new();
    // Unlit tells engines to skip the lighting path — emission included — so
    // an emissive material (crystals, lamps) stays on the lit path, where its
    // glow is honoured; its base color already reads as the glow color.
    if mat.emissive.is_none() {
        ext.insert("KHR_materials_unlit".into(), json!({}));
    }
    if let Some(e) = mat.emissive {
        m.insert("emissiveFactor".into(), json!(e.map(|x| x.clamp(0.0, 1.0))));
        if (mat.emissive_strength - 1.0).abs() > f32::EPSILON {
            ext.insert(
                "KHR_materials_emissive_strength".into(),
                json!({ "emissiveStrength": mat.emissive_strength }),
            );
        }
    }
    if !ext.is_empty() {
        m.insert("extensions".into(), Value::Object(ext));
    }
    Ok(Value::Object(m))
}

/// Welded vertex streams + triangle indices for one object.
struct Prim {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    uvs: Vec<[f32; 2]>,
    colors: Vec<[f32; 3]>,
    joints: Vec<[u8; 4]>,
    weights: Vec<[f32; 4]>,
    indices: Vec<u32>,
}

/// Weld key: exact bit patterns of (pos, normal, uv) plus the resolved skin
/// influence. Unrigged objects have a constant skin field, so the weld is
/// purely (pos, normal, uv); on rigged objects the influence keeps verts
/// with different weights apart.
type WeldKey = ([u32; 3], [u32; 3], [u32; 2], [u8; 4], [u32; 4]);

fn weld(mesh: &Mesh, hard_angle_deg: f32, rig: Option<&Rig>) -> Prim {
    let corner_normals = mesh.corner_normals(hard_angle_deg);
    let mut lookup: BTreeMap<WeldKey, u32> = BTreeMap::new();
    let mut corner_index: BTreeMap<(FaceId, u16), u32> = BTreeMap::new();
    let mut prim = Prim {
        positions: Vec::new(),
        normals: Vec::new(),
        uvs: Vec::new(),
        colors: Vec::new(),
        joints: Vec::new(),
        weights: Vec::new(),
        indices: Vec::new(),
    };
    for (&fid, face) in &mesh.faces {
        for (ci, corner) in face.corners.iter().enumerate() {
            let pos = mesh.verts[&corner.vert];
            let normal = corner_normals[&(fid, ci as u16)];
            let (joints, weights) = match rig {
                Some(rig) => influences(rig, corner.vert),
                None => ([0; 4], [0.0; 4]),
            };
            let key: WeldKey = (
                pos.to_array().map(f32::to_bits),
                normal.to_array().map(f32::to_bits),
                corner.uv.to_array().map(f32::to_bits),
                joints,
                weights.map(f32::to_bits),
            );
            let idx = *lookup.entry(key).or_insert_with(|| {
                prim.positions.push(pos);
                prim.normals.push(normal);
                prim.uvs.push(corner.uv.to_array());
                prim.colors.push(mesh.colors.get(&corner.vert).copied().unwrap_or([1.0; 3]));
                prim.joints.push(joints);
                prim.weights.push(weights);
                (prim.positions.len() - 1) as u32
            });
            corner_index.insert((fid, ci as u16), idx);
        }
    }
    for tri in mesh.triangulate() {
        for c in tri.corner_idx {
            prim.indices.push(corner_index[&(tri.face, c)]);
        }
    }
    prim
}

/// Top-4 influences by weight (ties broken by bone index), renormalized to
/// sum to 1. Uninfluenced verts bind fully to joint 0 so the accessor stays
/// valid; the rig gate flags them upstream. Bone indices fit u8: the rig
/// gate caps skeletons at 40 bones.
fn influences(rig: &Rig, v: VertId) -> ([u8; 4], [f32; 4]) {
    let mut list: Vec<(u16, f32)> =
        rig.weights.get(&v).map(|w| w.iter().copied().filter(|&(_, w)| w > 0.0).collect()).unwrap_or_default();
    list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    list.truncate(4);
    if list.is_empty() {
        list.push((0, 1.0));
    }
    let sum: f32 = list.iter().map(|&(_, w)| w).sum();
    let mut joints = [0u8; 4];
    let mut weights = [0f32; 4];
    for (slot, &(j, w)) in list.iter().enumerate() {
        joints[slot] = j.min(u8::MAX as u16) as u8;
        weights[slot] = w / sum;
    }
    (joints, weights)
}

/// GLB container: 12-byte header, JSON chunk padded with spaces, BIN chunk
/// padded with zeros.
fn pack_glb(mut json: Vec<u8>, mut bin: Vec<u8>) -> Vec<u8> {
    while json.len() % 4 != 0 {
        json.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0);
    }
    let bin_part = if bin.is_empty() { 0 } else { 8 + bin.len() };
    let total = 12 + 8 + json.len() + bin_part;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6C67_u32.to_le_bytes()); // "glTF"
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534A_u32.to_le_bytes()); // "JSON"
    out.extend_from_slice(&json);
    if !bin.is_empty() {
        out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
        out.extend_from_slice(&0x004E_4942_u32.to_le_bytes()); // "BIN\0"
        out.extend_from_slice(&bin);
    }
    out
}
