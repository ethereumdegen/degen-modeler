//! Scene extraction shared by views, metrics and raycast: world triangles,
//! wire edges, per-face areas/densities and resolved textures.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use dgm_atlas::Pack;
use dgm_mesh::{EdgeKey, Face, FaceId, Mesh};
use dgm_scene::{AlphaMode, Doc, Object, TextureRef};
use glam::{Vec2, Vec3};
use image::RgbaImage;

/// Fallback for a File/Trim texture whose bytes are not on disk (yet).
pub(crate) const MISSING_GRAY: [u8; 4] = [0x80, 0x80, 0x80, 0xff];
/// Objects with no material at all.
pub(crate) const UNTEXTURED: [u8; 4] = [0xb4, 0xb4, 0xb4, 0xff];

pub(crate) enum Tex {
    Image(RgbaImage),
    Flat([u8; 4]),
}

impl Tex {
    /// Nearest-neighbour sample with wrap; UV origin top-left.
    pub fn sample(&self, uv: Vec2) -> [u8; 4] {
        match self {
            Tex::Flat(c) => *c,
            Tex::Image(img) => {
                let (w, h) = (img.width(), img.height());
                let u = uv.x - uv.x.floor();
                let v = uv.y - uv.y.floor();
                let x = ((u * w as f32) as u32).min(w - 1);
                let y = ((v * h as f32) as u32).min(h - 1);
                img.get_pixel(x, y).0
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TexKind {
    Color,
    Trim,
    File,
    Untextured,
}

/// One world-space triangle of a fan-triangulated face.
pub(crate) struct TriGeom {
    pub face: FaceId,
    pub pos: [Vec3; 3],
    pub uv: [Vec2; 3],
    pub normal: Vec3,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct FaceInfo {
    pub world_area: f32,
    pub uv_area: f32,
    /// Texel density in texels/meter; `None` without a known texture size
    /// or with a degenerate face.
    pub density: Option<f32>,
}

pub(crate) struct ObjGeom {
    pub name: String,
    pub is_lod: bool,
    pub kind: TexKind,
    pub tex: Tex,
    pub alpha_mask: bool,
    pub missing_texture: bool,
    pub tris: Vec<TriGeom>,
    /// Face boundary edges in corner order (wires; duplicates on shared
    /// edges are harmless).
    pub wires: Vec<(Vec3, Vec3)>,
    pub faces: BTreeMap<FaceId, FaceInfo>,
}

pub(crate) struct SceneGeom {
    pub objects: Vec<ObjGeom>,
    pub bounds: Option<(Vec3, Vec3)>,
}

impl SceneGeom {
    /// Bounding-sphere framing: (center, radius). Empty scenes frame a unit
    /// box so the checker background still renders.
    pub fn framing(&self) -> (Vec3, f32) {
        match self.bounds {
            Some((lo, hi)) => ((lo + hi) * 0.5, ((hi - lo).length() * 0.5).max(1e-3)),
            None => (Vec3::ZERO, 0.87),
        }
    }
}

fn load_image(path: &Path) -> Option<RgbaImage> {
    let img = image::ImageReader::open(path).ok()?.decode().ok()?.to_rgba8();
    (img.width() > 0 && img.height() > 0).then_some(img)
}

struct ResolvedTex {
    tex: Tex,
    kind: TexKind,
    alpha_mask: bool,
    missing: bool,
    /// Texture pixel count (w*h) for density math, when known.
    px2: Option<f32>,
}

/// A `File` texture path is project-relative; the project's pack copy lives
/// at `<project>/pack`, so the pack's parent is the project root —
/// deterministic resolution, independent of the process cwd.
pub(crate) fn file_path(pack: &Pack, path: &str) -> std::path::PathBuf {
    match pack.root.parent() {
        Some(root) => root.join(path),
        None => std::path::PathBuf::from(path),
    }
}

fn resolve_tex(doc: &Doc, pack: &Pack, obj: &Object) -> ResolvedTex {
    let Some(mat) = obj.material.as_deref().and_then(|m| doc.materials.get(m)) else {
        return ResolvedTex {
            tex: Tex::Flat(UNTEXTURED),
            kind: TexKind::Untextured,
            alpha_mask: false,
            missing: false,
            px2: None,
        };
    };
    let alpha_mask = mat.alpha == AlphaMode::Mask;
    match &mat.texture {
        TextureRef::Color { rgba } => ResolvedTex {
            tex: Tex::Flat(*rgba),
            kind: TexKind::Color,
            alpha_mask,
            missing: false,
            px2: None,
        },
        TextureRef::Trim { sheet } => match pack.manifest.trims.get(sheet) {
            None => ResolvedTex {
                tex: Tex::Flat(MISSING_GRAY),
                kind: TexKind::Trim,
                alpha_mask,
                missing: true,
                px2: None,
            },
            Some(trim) => {
                // Density uses the manifest size even when the PNG is
                // absent, so the metric stays meaningful pre-sync.
                let px2 = Some((trim.size[0] * trim.size[1]) as f32);
                match load_image(&pack.root.join(&trim.file)) {
                    Some(img) => ResolvedTex {
                        tex: Tex::Image(img),
                        kind: TexKind::Trim,
                        alpha_mask,
                        missing: false,
                        px2,
                    },
                    None => ResolvedTex {
                        tex: Tex::Flat(MISSING_GRAY),
                        kind: TexKind::Trim,
                        alpha_mask,
                        missing: true,
                        px2,
                    },
                }
            }
        },
        // Project-relative; the pack copy lives at <project>/pack, so the
        // pack's parent IS the project root — deterministic, cwd-free.
        TextureRef::File { path } => match load_image(&file_path(pack, path)) {
            Some(img) => {
                let px2 = Some((img.width() * img.height()) as f32);
                ResolvedTex { tex: Tex::Image(img), kind: TexKind::File, alpha_mask, missing: false, px2 }
            }
            None => ResolvedTex {
                tex: Tex::Flat(MISSING_GRAY),
                kind: TexKind::File,
                alpha_mask,
                missing: true,
                px2: None,
            },
        },
    }
}

/// Shoelace area of a face's UV polygon.
pub(crate) fn uv_poly_area(face: &Face) -> f32 {
    let n = face.corners.len();
    let mut s = 0.0;
    for i in 0..n {
        let a = face.corners[i].uv;
        let b = face.corners[(i + 1) % n].uv;
        s += a.x * b.y - b.x * a.y;
    }
    (s * 0.5).abs()
}

fn object_geom(doc: &Doc, pack: &Pack, name: &str, obj: &Object) -> ObjGeom {
    let rt = resolve_tex(doc, pack, obj);
    let mesh = &obj.mesh;
    let mut tris = Vec::with_capacity(mesh.tri_count() as usize);
    let mut wires = Vec::new();
    let mut faces = BTreeMap::new();
    for (&fid, face) in &mesh.faces {
        let normal = mesh.face_normal(fid).unwrap_or(Vec3::Y);
        let n = face.corners.len();
        for i in 0..n {
            wires.push((
                mesh.verts[&face.corners[i].vert],
                mesh.verts[&face.corners[(i + 1) % n].vert],
            ));
        }
        for i in 1..n - 1 {
            let c = [&face.corners[0], &face.corners[i], &face.corners[i + 1]];
            tris.push(TriGeom {
                face: fid,
                pos: c.map(|c| mesh.verts[&c.vert]),
                uv: c.map(|c| c.uv),
                normal,
            });
        }
        let world_area = mesh.face_area(fid).unwrap_or(0.0);
        let uv_area = uv_poly_area(face);
        let density = rt
            .px2
            .filter(|_| world_area > 1e-12 && uv_area > 1e-12)
            .map(|px2| (uv_area * px2 / world_area).sqrt());
        faces.insert(fid, FaceInfo { world_area, uv_area, density });
    }
    ObjGeom {
        name: name.to_string(),
        is_lod: obj.lod_of.is_some(),
        kind: rt.kind,
        tex: rt.tex,
        alpha_mask: rt.alpha_mask,
        missing_texture: rt.missing,
        tris,
        wires,
        faces,
    }
}

/// Extract every object (doc order). Beauty views pass `include_lods =
/// false` so LOD meshes don't z-fight their source.
pub(crate) fn scene_geom(doc: &Doc, pack: &Pack, include_lods: bool) -> SceneGeom {
    let mut objects = Vec::new();
    let mut bounds: Option<(Vec3, Vec3)> = None;
    for (name, obj) in &doc.objects {
        if !include_lods && obj.lod_of.is_some() {
            continue;
        }
        if let Some((lo, hi)) = obj.mesh.bounds() {
            bounds = Some(match bounds {
                Some((l, h)) => (l.min(lo), h.max(hi)),
                None => (lo, hi),
            });
        }
        objects.push(object_geom(doc, pack, name, obj));
    }
    SceneGeom { objects, bounds }
}

/// World-space fan triangles of one mesh (silhouettes, raycast fixtures).
pub(crate) fn world_tris(mesh: &Mesh) -> Vec<[Vec3; 3]> {
    let mut out = Vec::with_capacity(mesh.tri_count() as usize);
    for face in mesh.faces.values() {
        for i in 1..face.corners.len() - 1 {
            out.push([
                mesh.verts[&face.corners[0].vert],
                mesh.verts[&face.corners[i].vert],
                mesh.verts[&face.corners[i + 1].vert],
            ]);
        }
    }
    out
}

/// UV of `e.0` and `e.1` within `face`, if the face uses both verts.
pub(crate) fn edge_uvs(face: &Face, e: EdgeKey) -> Option<(Vec2, Vec2)> {
    let a = face.corners.iter().find(|c| c.vert == e.0)?.uv;
    let b = face.corners.iter().find(|c| c.vert == e.1)?.uv;
    Some((a, b))
}

/// UV seam edges: explicitly marked seams plus interior edges whose two
/// faces disagree about the shared UVs.
pub(crate) fn seam_edges(mesh: &Mesh) -> BTreeSet<EdgeKey> {
    let mut out: BTreeSet<EdgeKey> = mesh.seams.iter().copied().collect();
    for (edge, faces) in mesh.edge_faces() {
        if faces.len() != 2 || out.contains(&edge) {
            continue;
        }
        if let (Some(a), Some(b)) = (
            edge_uvs(&mesh.faces[&faces[0]], edge),
            edge_uvs(&mesh.faces[&faces[1]], edge),
        ) && ((a.0 - b.0).length_squared() > 1e-10 || (a.1 - b.1).length_squared() > 1e-10)
        {
            out.insert(edge);
        }
    }
    out
}
