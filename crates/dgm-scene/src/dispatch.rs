//! Op dispatch: one match, one behaviour per op, no side channels.

use std::collections::BTreeSet;

use dgm_atlas::Pack;
use dgm_mesh::ops::{deform as mesh_deform, topo};
use dgm_mesh::{EdgeKey, Mesh, VertId, primitives};
use glam::{Mat4, Quat, Vec3};

use crate::doc::{Doc, Material, MirrorSet, Object, TextureRef};
use crate::error::OpError;
use crate::op::{Axis, Diff, Op};
use crate::select::{self, SelRef};

pub fn apply(doc: &mut Doc, pack: &Pack, op: &Op) -> Result<Diff, OpError> {
    match op {
        // ---- primitives ----
        Op::PrimBox { object, size } => {
            let mesh = primitives::prim_box(Vec3::from(*size))?;
            insert_object(doc, object, mesh)
        }
        Op::PrimPlane { object, size } => {
            let mesh = primitives::plane(size[0], size[1])?;
            insert_object(doc, object, mesh)
        }
        Op::PrimCylinder { object, radius, height, segments, caps } => {
            let mesh = primitives::cylinder(*radius, *height, *segments, *caps)?;
            insert_object(doc, object, mesh)
        }
        Op::PrimLathe { object, profile, segments, caps } => {
            let mesh = primitives::lathe(profile, *segments, *caps)?;
            insert_object(doc, object, mesh)
        }
        Op::PrimNgonPrism { object, sides, radius, height } => {
            let mesh = primitives::ngon_prism(*sides, *radius, *height)?;
            insert_object(doc, object, mesh)
        }

        // ---- deform (selection-level) ----
        Op::Translate { sel, delta, proportional } => {
            deform(doc, sel, *proportional, |_| Mat4::from_translation(Vec3::from(*delta)))
        }
        Op::Rotate { sel, axis, degrees, origin, proportional } => {
            let (axis, degrees, origin) = (*axis, *degrees, *origin);
            deform(doc, sel, *proportional, move |centroid| {
                let o = origin.map(Vec3::from).unwrap_or(centroid);
                Mat4::from_translation(o)
                    * Mat4::from_quat(Quat::from_axis_angle(axis.unit(), degrees.to_radians()))
                    * Mat4::from_translation(-o)
            })
        }
        Op::Scale { sel, factors, origin, proportional } => {
            let (factors, origin) = (*factors, *origin);
            deform(doc, sel, *proportional, move |centroid| {
                let o = origin.map(Vec3::from).unwrap_or(centroid);
                Mat4::from_translation(o)
                    * Mat4::from_scale(Vec3::from(factors))
                    * Mat4::from_translation(-o)
            })
        }
        Op::SnapToGrid { sel, step } => {
            if *step <= 0.0 {
                return Err(OpError::BadParams("snap_to_grid: step must be > 0".into()));
            }
            let selection = select::resolve(doc, sel)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let verts = select::to_verts(mesh, &selection)?;
            for &v in &verts {
                let p = mesh.pos(v)?;
                mesh.set_pos(v, (p / *step).round() * *step)?;
            }
            Ok(Diff {
                summary: format!("{object}: snapped {} verts to {step}m grid", verts.len()),
                modified: verts.iter().map(|v| v.to_string()).collect(),
                ..Diff::default()
            })
        }

        Op::FlipNormals { sel } => {
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            for &f in &faces {
                mesh.face_mut(f)?.corners.reverse();
            }
            Ok(Diff {
                summary: format!("{object}: flipped {} faces", faces.len()),
                modified: faces.iter().map(|f| f.to_string()).collect(),
                ..Diff::default()
            })
        }

        // ---- selection ----
        Op::SelectSave { name, query } => {
            let selection = select::resolve_query(doc, query)?;
            let n = selection.elems.len();
            doc.selections.insert(name.clone(), selection);
            Ok(Diff::summary(format!("selection `{name}`: {n} elems")))
        }

        // ---- seams / mirror declarations ----
        Op::MarkSeams { sel } | Op::ClearSeams { sel } => {
            let mark = matches!(op, Op::MarkSeams { .. });
            let selection = select::resolve(doc, sel)?;
            let edges = select::to_edges(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            for &e in &edges {
                if mark {
                    mesh.seams.insert(e);
                } else {
                    mesh.seams.remove(&e);
                }
            }
            let verb = if mark { "marked" } else { "cleared" };
            Ok(Diff {
                summary: format!("{object}: {verb} {} seam edges", edges.len()),
                modified: edges.iter().map(|e| e.to_string()).collect(),
                ..Diff::default()
            })
        }
        Op::UvDeclareMirror { name, sel } => {
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let n = faces.len();
            doc.mirror_sets
                .insert(name.clone(), MirrorSet { object: selection.object, faces });
            Ok(Diff::summary(format!("mirror set `{name}`: {n} faces")))
        }

        // ---- material ----
        Op::MaterialNew { name, texture, alpha, double_sided } => {
            if let TextureRef::Trim { sheet } = texture
                && !pack.manifest.trims.contains_key(sheet)
            {
                return Err(OpError::Pack(format!("no trim sheet `{sheet}` in pack")));
            }
            doc.materials.insert(
                name.clone(),
                Material { texture: texture.clone(), alpha: *alpha, double_sided: *double_sided },
            );
            Ok(Diff::summary(format!("material `{name}`")))
        }
        Op::ObjectMaterial { object, material } => {
            if !doc.materials.contains_key(material) {
                return Err(OpError::UnknownMaterial(material.clone()));
            }
            doc.object_mut(object)?.material = Some(material.clone());
            Ok(Diff::summary(format!("{object}: material `{material}`")))
        }

        // ---- scene ----
        Op::ObjectDelete { object } => {
            doc.object(object)?;
            let lods: Vec<String> = doc
                .objects
                .iter()
                .filter(|(_, o)| o.lod_of.as_deref() == Some(object))
                .map(|(n, _)| n.clone())
                .collect();
            doc.objects.remove(object);
            for lod in &lods {
                doc.objects.remove(lod);
            }
            doc.selections.retain(|_, s| &s.object != object && !lods.contains(&s.object));
            doc.mirror_sets.retain(|_, m| &m.object != object && !lods.contains(&m.object));
            doc.clips.retain(|_, c| &c.object != object && !lods.contains(&c.object));
            let mut removed = vec![format!("object:{object}")];
            removed.extend(lods.iter().map(|l| format!("object:{l}")));
            Ok(Diff {
                summary: format!("deleted `{object}` (+{} lods)", lods.len()),
                removed,
                ..Diff::default()
            })
        }
        Op::ObjectRename { from, to } => {
            if doc.objects.contains_key(to) {
                return Err(OpError::ObjectExists(to.clone()));
            }
            let object = doc.objects.remove(from).ok_or_else(|| OpError::UnknownObject(from.clone()))?;
            doc.objects.insert(to.clone(), object);
            for s in doc.selections.values_mut() {
                if &s.object == from {
                    s.object = to.clone();
                }
            }
            for m in doc.mirror_sets.values_mut() {
                if &m.object == from {
                    m.object = to.clone();
                }
            }
            for c in doc.clips.values_mut() {
                if &c.object == from {
                    c.object = to.clone();
                }
            }
            for o in doc.objects.values_mut() {
                if o.lod_of.as_deref() == Some(from.as_str()) {
                    o.lod_of = Some(to.clone());
                }
            }
            Ok(Diff::summary(format!("renamed `{from}` -> `{to}`")))
        }
        Op::ObjectInstance { src, dst, translate } => {
            if doc.objects.contains_key(dst) {
                return Err(OpError::ObjectExists(dst.clone()));
            }
            let source = doc.object(src)?;
            let mut mesh = source.mesh.clone();
            let material = source.material.clone();
            let all: BTreeSet<VertId> = mesh.verts.keys().copied().collect();
            mesh.transform_verts(&all, &Mat4::from_translation(Vec3::from(*translate)));
            let mut object = Object::new(mesh);
            object.material = material;
            doc.objects.insert(dst.clone(), object);
            Ok(Diff {
                summary: format!("instanced `{src}` -> `{dst}`"),
                created: vec![format!("object:{dst}")],
                ..Diff::default()
            })
        }
        Op::SetGoal { goal } => {
            doc.goal = Some(goal.clone());
            Ok(Diff::summary("goal set"))
        }
        Op::SetClass { class } => {
            doc.asset_class = *class;
            Ok(Diff::summary(format!("asset class: {class}")))
        }

        // ---- topology (dgm-mesh::ops) ----
        Op::Extrude { sel, offset } => {
            on_faces(doc, sel, |mesh, faces| topo::extrude(mesh, faces, *offset))
        }
        Op::Inset { sel, thickness, depth } => {
            on_faces(doc, sel, |mesh, faces| topo::inset(mesh, faces, *thickness, *depth))
        }
        Op::Bevel { sel, width, segments } => {
            let selection = select::resolve(doc, sel)?;
            let edges = select::to_edges(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = topo::bevel_edges(mesh, &edges, *width, *segments)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::LoopCut { object, edge, cuts } => {
            let mesh = &mut doc.object_mut(object)?.mesh;
            let e = EdgeKey::new(VertId(edge[0]), VertId(edge[1]));
            let delta = topo::loop_cut(mesh, e, *cuts)?;
            Ok(Diff::from_delta(object, &delta))
        }
        Op::Bridge { sel_a, sel_b } => {
            let a = select::resolve(doc, sel_a)?;
            let b = select::resolve(doc, sel_b)?;
            if a.object != b.object {
                return Err(OpError::BadParams(format!(
                    "bridge: selections span objects `{}` and `{}`",
                    a.object, b.object
                )));
            }
            let (ea, eb) = (select::to_edges(&a)?, select::to_edges(&b)?);
            let object = a.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = topo::bridge(mesh, &ea, &eb)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::MergeVerts { sel, distance } => {
            let selection = select::resolve(doc, sel)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let verts = select::to_verts(mesh, &selection)?;
            let delta = topo::merge_verts(mesh, &verts, *distance)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::Dissolve { sel } => on_faces(doc, sel, |mesh, faces| topo::dissolve(mesh, faces)),
        Op::Mirror { object, axis, merge_distance } => {
            let axis = match axis {
                Axis::X => 0,
                Axis::Y => 1,
                Axis::Z => 2,
            };
            let mesh = &mut doc.object_mut(object)?.mesh;
            let delta = topo::mirror(mesh, axis, *merge_distance)?;
            Ok(Diff::from_delta(object, &delta))
        }
        Op::DecimateToBudget { object, target_tris } => {
            let mesh = &mut doc.object_mut(object)?.mesh;
            let delta = topo::decimate_to_target(mesh, *target_tris)?;
            Ok(Diff::from_delta(object, &delta))
        }
        Op::Lattice { object, dims, displacements } => {
            let d: Vec<Vec3> = displacements.iter().copied().map(Vec3::from).collect();
            let mesh = &mut doc.object_mut(object)?.mesh;
            let delta = mesh_deform::lattice(mesh, *dims, &d)?;
            Ok(Diff::from_delta(object, &delta))
        }
        Op::LodGenerate { object, ratios } => {
            if ratios.is_empty() || ratios.iter().any(|r| !(*r > 0.0 && *r < 1.0)) {
                return Err(OpError::BadParams(
                    "lod_generate: ratios must be in (0, 1)".into(),
                ));
            }
            let source = doc.object(object)?.clone();
            let base_tris = source.mesh.tri_count();
            let mut created = Vec::new();
            let mut lods = Vec::new();
            for (i, ratio) in ratios.iter().enumerate() {
                let name = format!("{object}_lod{}", i + 1);
                if doc.objects.contains_key(&name) {
                    return Err(OpError::ObjectExists(name));
                }
                let mut mesh = source.mesh.clone();
                let target = ((base_tris as f32 * ratio).round() as u32).max(4);
                topo::decimate_to_target(&mut mesh, target)?;
                let mut lod = Object::new(mesh);
                lod.material = source.material.clone();
                lod.lod_of = Some(object.clone());
                lod.lod_level = (i + 1) as u8;
                created.push(format!("object:{name}"));
                lods.push((name, lod));
            }
            for (name, lod) in lods {
                doc.objects.insert(name, lod);
            }
            Ok(Diff {
                summary: format!("{object}: {} LODs from {base_tris} tris", ratios.len()),
                created,
                ..Diff::default()
            })
        }

        // ---- uv (dgm-uv) ----
        Op::UvUnwrap { object } => {
            let mesh = &mut doc.object_mut(object)?.mesh;
            let delta = dgm_uv::unwrap(mesh)?;
            Ok(Diff::from_delta(object, &delta))
        }
        Op::UvProject { sel, kind, axis } => {
            use crate::op::Projection;
            let to_uv = |axis: Axis| match axis {
                Axis::X => dgm_uv::Axis::X,
                Axis::Y => dgm_uv::Axis::Y,
                Axis::Z => dgm_uv::Axis::Z,
            };
            let need_axis = || {
                axis.ok_or_else(|| {
                    OpError::BadParams(format!("uv_project {:?}: `axis` is required", kind))
                })
            };
            let kind = match kind {
                Projection::Planar => dgm_uv::ProjectKind::Planar { axis: to_uv(need_axis()?) },
                Projection::Cylindrical => {
                    dgm_uv::ProjectKind::Cylindrical { axis: to_uv(need_axis()?) }
                }
                Projection::Box => {
                    if axis.is_some() {
                        return Err(OpError::BadParams(
                            "uv_project box: `axis` does not apply".into(),
                        ));
                    }
                    dgm_uv::ProjectKind::Box
                }
            };
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = dgm_uv::project(mesh, &faces, &kind)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::UvAssignTrim { sel, sheet, region } => {
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = dgm_uv::assign_trim(mesh, &faces, pack, sheet, region)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::UvAssignRect { sel, rect } => {
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let object = selection.object.clone();
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = dgm_uv::assign_rect(mesh, &faces, *rect)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::UvSetTexelDensity { sel, texels_per_meter } => {
            let selection = select::resolve(doc, sel)?;
            let faces = select::to_faces(&selection)?;
            let object = selection.object.clone();
            let px = texture_px(doc, pack, &object)?;
            let mesh = &mut doc.object_mut(&object)?.mesh;
            let delta = dgm_uv::set_texel_density(mesh, &faces, *texels_per_meter, px)?;
            Ok(Diff::from_delta(&object, &delta))
        }
        Op::UvPack { object, margin_px } => {
            let px = texture_px(doc, pack, object)?;
            let mesh = &mut doc.object_mut(object)?.mesh;
            let delta = dgm_uv::pack_islands(mesh, *margin_px, px)?;
            Ok(Diff::from_delta(object, &delta))
        }

        // ---- rig / anim (rig_ops, anim_ops) ----
        Op::RigApply { object, preset, fit } => {
            crate::rig_ops::rig_apply(doc, pack, object, preset, fit)
        }
        Op::RigAutoWeights { object } => crate::rig_ops::rig_auto_weights(doc, object),
        Op::RigPaintWeights { sel, bone, value, falloff, mode } => {
            let selection = select::resolve(doc, sel)?;
            crate::rig_ops::rig_paint_weights(doc, &selection, bone, *value, *falloff, *mode)
        }
        Op::RigAddBone { object, name, parent, head, tail } => {
            crate::rig_ops::rig_add_bone(doc, object, name, parent, *head, *tail)
        }
        Op::ClipApply { name, object, template, speed } => {
            crate::anim_ops::clip_apply(doc, pack, name, object, template, *speed)
        }
        Op::ClipKey { clip, bone, time, rotation_euler_deg, translation, scale } => {
            crate::anim_ops::clip_key(
                doc,
                clip,
                bone,
                *time,
                *rotation_euler_deg,
                *translation,
                *scale,
            )
        }
        Op::ClipSetLoop { clip, looped } => crate::anim_ops::clip_set_loop(doc, clip, *looped),
        Op::ClipDelete { clip } => crate::anim_ops::clip_delete(doc, clip),
    }
}

fn insert_object(doc: &mut Doc, name: &str, mesh: Mesh) -> Result<Diff, OpError> {
    if doc.objects.contains_key(name) {
        return Err(OpError::ObjectExists(name.into()));
    }
    let (verts, faces) = (mesh.verts.len(), mesh.faces.len());
    doc.objects.insert(name.into(), Object::new(mesh));
    Ok(Diff {
        summary: format!("{name}: {verts} verts, {faces} faces"),
        created: vec![format!("object:{name}")],
        ..Diff::default()
    })
}

/// Face-selection topology op: resolve, narrow to faces, run, diff.
fn on_faces(
    doc: &mut Doc,
    sel: &SelRef,
    run: impl FnOnce(
        &mut Mesh,
        &BTreeSet<dgm_mesh::FaceId>,
    ) -> Result<dgm_mesh::MeshDelta, dgm_mesh::MeshError>,
) -> Result<Diff, OpError> {
    let selection = select::resolve(doc, sel)?;
    let faces = select::to_faces(&selection)?;
    let object = selection.object.clone();
    let mesh = &mut doc.object_mut(&object)?.mesh;
    let delta = run(mesh, &faces)?;
    Ok(Diff::from_delta(&object, &delta))
}

/// Texture pixel size an object's UVs answer to: the bound trim sheet's
/// larger edge; `File` textures use the pack's largest allowed size (their
/// pixel size is not knowable in the I/O-free dispatch); `Color`/no
/// material cannot host density-aware UV ops.
fn texture_px(doc: &Doc, pack: &Pack, object: &str) -> Result<u32, OpError> {
    let material = doc
        .object(object)?
        .material
        .as_ref()
        .and_then(|m| doc.materials.get(m));
    match material.map(|m| &m.texture) {
        Some(TextureRef::Trim { sheet }) => pack
            .manifest
            .trims
            .get(sheet)
            .map(|t| t.size[0].max(t.size[1]))
            .ok_or_else(|| OpError::Pack(format!("no trim sheet `{sheet}` in pack"))),
        Some(TextureRef::File { .. }) => {
            Ok(pack.manifest.texture_sizes.iter().copied().max().unwrap_or(512))
        }
        Some(TextureRef::Color { .. }) | None => Err(OpError::BadParams(format!(
            "object `{object}`: texel-density ops need a textured material bound first"
        ))),
    }
}

/// Shared deform path: resolve the selection to vertices, optionally spread
/// the transform over neighbours with a cosine falloff of `radius` meters.
fn deform(
    doc: &mut Doc,
    sel: &SelRef,
    proportional: Option<f32>,
    make: impl Fn(Vec3) -> Mat4,
) -> Result<Diff, OpError> {
    let selection = select::resolve(doc, sel)?;
    let object = selection.object.clone();
    let mesh = &mut doc.object_mut(&object)?.mesh;
    let verts = select::to_verts(mesh, &selection)?;
    if verts.is_empty() {
        return Err(OpError::BadParams("empty selection".into()));
    }

    let mut centroid = Vec3::ZERO;
    for &v in &verts {
        centroid += mesh.pos(v)?;
    }
    centroid /= verts.len() as f32;
    let m = make(centroid);

    let mut touched: Vec<VertId> = verts.iter().copied().collect();
    match proportional {
        None => mesh.transform_verts(&verts, &m),
        Some(radius) => {
            if radius <= 0.0 {
                return Err(OpError::BadParams("proportional radius must be > 0".into()));
            }
            let anchors: Vec<Vec3> = verts.iter().map(|&v| mesh.pos(v).unwrap()).collect();
            let others: Vec<VertId> = mesh.verts.keys().copied().filter(|v| !verts.contains(v)).collect();
            for v in others {
                let p = mesh.pos(v)?;
                let d = anchors
                    .iter()
                    .map(|a| a.distance(p))
                    .fold(f32::INFINITY, f32::min);
                if d < radius {
                    let w = 0.5 + 0.5 * (std::f32::consts::PI * d / radius).cos();
                    mesh.set_pos(v, p.lerp(m.transform_point3(p), w))?;
                    touched.push(v);
                }
            }
            mesh.transform_verts(&verts, &m);
        }
    }
    Ok(Diff {
        summary: format!("{object}: moved {} verts", touched.len()),
        modified: touched.iter().map(|v| v.to_string()).collect(),
        ..Diff::default()
    })
}
