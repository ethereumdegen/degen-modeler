//! Ledger polling (1s mtime check -> doc reload) and viewport mesh sync.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::PrimitiveTopology;
use bevy::prelude::*;
use dgm_atlas::Pack;
use dgm_scene::doc::TextureRef;
use dgm_scene::project::Project;
use dgm_scene::{AlphaMode as DocAlphaMode, Doc};

use crate::feed::{newest_review_name, parse_review, parse_ticker_tail};
use crate::state::{DigestCache, ObjectView, Review, Ticker, UiProject, Views, compute_digest, ledger_mtime};
use crate::viewport::{BuiltObject, build_object, object_signature};

pub const TICKER_KEEP: usize = 14;

/// Every second: reload the doc when `ops.jsonl` mtime moved, refresh the
/// op ticker and the newest review artifact.
pub fn poll_ledger(
    time: Res<Time>,
    mut up: ResMut<UiProject>,
    mut ticker: ResMut<Ticker>,
    mut review: ResMut<Review>,
) {
    if !up.poll.tick(time.delta()).just_finished() {
        return;
    }

    let mtime = ledger_mtime(&up.project);
    if mtime != up.ledger_mtime {
        up.ledger_mtime = mtime;
        match Project::load(&up.project.root) {
            Ok(project) => {
                up.project = project;
                up.dirty = true;
                up.status = None;
            }
            // Torn write mid-append: keep the old doc, retry next poll.
            Err(e) => up.status = Some(format!("reload failed: {e}")),
        }
    }

    if let Ok(text) = std::fs::read_to_string(up.project.ledger_path()) {
        ticker.lines = parse_ticker_tail(&text, TICKER_KEEP);
    }

    let artifacts = up.project.artifacts_dir();
    let names = std::fs::read_dir(&artifacts)
        .map(|rd| {
            rd.filter_map(|e| e.ok()?.file_name().into_string().ok())
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if let Some(name) = newest_review_name(names)
        && review.view.as_ref().map(|v| v.file.as_str()) != Some(name.as_str())
        && let Ok(text) = std::fs::read_to_string(artifacts.join(&name))
        && let Ok(value) = serde_json::from_str(&text)
    {
        review.view = Some(parse_review(&name, &value));
    }
}

/// Rebuild viewport meshes/materials for objects whose signature changed;
/// drop vanished objects; refresh the digest cache.
pub fn rebuild_meshes(
    mut up: ResMut<UiProject>,
    mut views: ResMut<Views>,
    mut digest: ResMut<DigestCache>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    asset_server: Res<AssetServer>,
    mut commands: Commands,
) {
    if !up.dirty {
        return;
    }
    up.dirty = false;
    let doc = &up.project.doc;
    let pack = &up.project.pack;
    let pack_rel = up.project.meta.pack.clone();

    let gone: Vec<String> = views
        .objects
        .keys()
        .filter(|name| !doc.objects.contains_key(*name))
        .cloned()
        .collect();
    for name in gone {
        if let Some(view) = views.objects.remove(&name) {
            commands.entity(view.entity).despawn();
        }
    }

    for (name, object) in &doc.objects {
        let sig = object_signature(doc, name);
        if views.objects.get(name).is_some_and(|v| v.sig == sig) {
            continue;
        }
        let built = build_object(&object.mesh, pack.manifest.hard_edge_angle_deg);
        let mesh_handle = meshes.add(bevy_mesh(&built));
        let material_handle = materials.add(doc_material(
            doc,
            pack,
            &pack_rel,
            object.material.as_deref(),
            &asset_server,
        ));
        match views.objects.get_mut(name) {
            Some(view) => {
                commands
                    .entity(view.entity)
                    .insert((Mesh3d(mesh_handle.clone()), MeshMaterial3d(material_handle)));
                view.mesh = mesh_handle;
                view.sig = sig;
                view.built = built;
            }
            None => {
                let entity = commands
                    .spawn((
                        Mesh3d(mesh_handle.clone()),
                        MeshMaterial3d(material_handle),
                        Transform::IDENTITY,
                        Name::new(name.clone()),
                    ))
                    .id();
                views
                    .objects
                    .insert(name.clone(), ObjectView { entity, mesh: mesh_handle, sig, built });
            }
        }
    }

    *digest = compute_digest(doc, pack);
}

fn bevy_mesh(built: &BuiltObject) -> Mesh {
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, built.positions.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, built.normals.clone());
    mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, built.uvs.clone());
    mesh
}

/// One unlit `StandardMaterial` per doc material; texture paths resolve
/// against the asset root (= project root).
fn doc_material(
    doc: &Doc,
    pack: &Pack,
    pack_rel: &str,
    name: Option<&str>,
    asset_server: &AssetServer,
) -> StandardMaterial {
    let mut mat = StandardMaterial { unlit: true, ..Default::default() };
    let Some(def) = name.and_then(|n| doc.materials.get(n)) else {
        // Untextured blockout grey.
        mat.base_color = Color::srgb(0.72, 0.72, 0.75);
        return mat;
    };
    match &def.texture {
        TextureRef::Trim { sheet } => match pack.manifest.trims.get(sheet) {
            Some(trim) => {
                mat.base_color_texture = Some(asset_server.load(format!("{pack_rel}/{}", trim.file)));
            }
            // Unknown sheet: loud magenta beats an invisible object.
            None => mat.base_color = Color::srgb(0.9, 0.1, 0.9),
        },
        TextureRef::File { path } => {
            mat.base_color_texture = Some(asset_server.load(path.clone()));
        }
        TextureRef::Color { rgba } => {
            mat.base_color = Color::srgba_u8(rgba[0], rgba[1], rgba[2], rgba[3]);
        }
    }
    if def.alpha == DocAlphaMode::Mask {
        mat.alpha_mode = AlphaMode::Mask(0.5);
    }
    if def.double_sided {
        mat.double_sided = true;
        mat.cull_mode = None;
    }
    mat
}
