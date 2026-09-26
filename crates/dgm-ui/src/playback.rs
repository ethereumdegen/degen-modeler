//! Skinned clip playback: sample the clip onto a shadow bone tree and
//! CPU-skin the viewport mesh each frame (fine at <=4k tris).

use bevy::prelude::*;

use crate::anim::{clip_time, skin_matrices, skin_vertex};
use crate::state::{Playback, UiProject, Views};
use crate::viewport::BuiltObject;

pub fn advance_playback(
    time: Res<Time>,
    up: Res<UiProject>,
    mut playback: ResMut<Playback>,
    views: Res<Views>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let doc = &up.project.doc;

    let Some(active) = playback.active.as_mut() else {
        if playback.restore {
            playback.restore = false;
            for view in views.objects.values() {
                if let Some(mut mesh) = meshes.get_mut(&view.mesh) {
                    write_streams(&mut mesh, view.built.positions.clone(), view.built.normals.clone());
                }
            }
        }
        return;
    };

    active.t += time.delta_secs();
    let (clip_name, playhead) = (active.clip.clone(), active.t);

    // Clip/object/rig can vanish under a live agent run; stop cleanly.
    let posed = doc.clips.get(&clip_name).and_then(|clip| {
        let rig = doc.objects.get(&clip.object)?.rig.as_ref()?;
        Some((clip, rig))
    });
    let Some((clip, rig)) = posed else {
        playback.active = None;
        playback.restore = true;
        return;
    };
    let Some(view) = views.objects.get(&clip.object) else { return };

    let t = clip_time(playhead, clip.duration, clip.looped);
    let mats = skin_matrices(rig, Some((clip, t)));
    let (positions, normals) = skin_streams(rig, &mats, &view.built);
    if let Some(mut mesh) = meshes.get_mut(&view.mesh) {
        write_streams(&mut mesh, positions, normals);
    }
}

fn skin_streams(
    rig: &dgm_scene::Rig,
    mats: &[glam::Mat4],
    built: &BuiltObject,
) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut positions = Vec::with_capacity(built.verts.len());
    let mut normals = Vec::with_capacity(built.verts.len());
    for i in 0..built.verts.len() {
        let (p, n) = skin_vertex(
            rig,
            mats,
            built.verts[i],
            glam::Vec3::from_array(built.positions[i]),
            glam::Vec3::from_array(built.normals[i]),
        );
        positions.push(p.to_array());
        normals.push(n.to_array());
    }
    (positions, normals)
}

fn write_streams(mesh: &mut Mesh, positions: Vec<[f32; 3]>, normals: Vec<[f32; 3]>) {
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, positions);
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, normals);
}
