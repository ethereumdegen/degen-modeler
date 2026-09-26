//! Shared ECS resources: the live project, viewport bookkeeping, panel state.

use std::collections::BTreeMap;
use std::time::SystemTime;

use bevy::prelude::*;
use dgm_atlas::Pack;
use dgm_mesh::{Severity, validate_mesh};
use dgm_scene::project::Project;
use dgm_scene::{Doc, budget_findings};

use crate::feed::{ReviewView, TickerLine};
use crate::viewport::BuiltObject;

/// The loaded project plus ledger-poll bookkeeping.
#[derive(Resource)]
pub struct UiProject {
    pub project: Project,
    pub ledger_mtime: Option<SystemTime>,
    pub poll: Timer,
    /// Last reload error, shown in the digest panel.
    pub status: Option<String>,
    /// Viewport meshes need a rebuild pass.
    pub dirty: bool,
}

impl UiProject {
    pub fn new(project: Project) -> Self {
        let ledger_mtime = ledger_mtime(&project);
        Self {
            project,
            ledger_mtime,
            poll: Timer::from_seconds(1.0, TimerMode::Repeating),
            status: None,
            dirty: true,
        }
    }
}

pub fn ledger_mtime(project: &Project) -> Option<SystemTime> {
    std::fs::metadata(project.ledger_path()).and_then(|m| m.modified()).ok()
}

/// One doc object's viewport state.
pub struct ObjectView {
    pub entity: Entity,
    pub mesh: Handle<Mesh>,
    pub sig: u64,
    pub built: BuiltObject,
}

#[derive(Resource, Default)]
pub struct Views {
    pub objects: BTreeMap<String, ObjectView>,
}

pub struct ObjRow {
    pub name: String,
    pub tris: u32,
    pub material: Option<String>,
    pub is_lod: bool,
}

/// Digest panel cache, recomputed whenever the doc changes.
#[derive(Resource, Default)]
pub struct DigestCache {
    pub revision: u64,
    pub class: String,
    pub rows: Vec<ObjRow>,
    pub total_tris: u32,
    pub budget: Option<u32>,
    pub hard: usize,
    pub warn: usize,
    pub hard_messages: Vec<String>,
}

/// Quick gate check for the badge: budget rules + per-mesh validation.
pub fn compute_digest(doc: &Doc, pack: &Pack) -> DigestCache {
    let mut findings = budget_findings(doc, pack);
    for (name, object) in &doc.objects {
        findings.extend(validate_mesh(&object.mesh).into_iter().map(|mut f| {
            f.message = format!("{name}: {}", f.message);
            f
        }));
    }
    let hard: Vec<&_> = findings.iter().filter(|f| f.severity == Severity::Hard).collect();
    let mut hard_messages: Vec<String> =
        hard.iter().take(6).map(|f| format!("{}: {}", f.rule, f.message)).collect();
    if hard.len() > 6 {
        hard_messages.push(format!("(+{} more)", hard.len() - 6));
    }
    DigestCache {
        revision: doc.revision,
        class: doc.asset_class.to_string(),
        rows: doc
            .objects
            .iter()
            .map(|(name, o)| ObjRow {
                name: name.clone(),
                tris: o.mesh.tri_count(),
                material: o.material.clone(),
                is_lod: o.lod_of.is_some(),
            })
            .collect(),
        total_tris: doc
            .objects
            .values()
            .filter(|o| o.lod_of.is_none())
            .map(|o| o.mesh.tri_count())
            .sum(),
        budget: pack.budget(doc.asset_class).map(|b| b.max_tris),
        hard: hard.len(),
        warn: findings.len() - hard.len(),
        hard_messages,
    }
}

pub struct ActiveClip {
    pub clip: String,
    pub t: f32,
}

#[derive(Resource, Default)]
pub struct Playback {
    pub active: Option<ActiveClip>,
    /// Rewrite rest-pose attributes once after playback stops.
    pub restore: bool,
}

#[derive(Resource, Default)]
pub struct Preview {
    pub requested: bool,
    pub entity: Option<Entity>,
    pub status: Option<String>,
}

#[derive(Resource, Default)]
pub struct Console {
    pub input: String,
    pub output: Option<String>,
}

#[derive(Resource, Default)]
pub struct Ticker {
    pub lines: Vec<TickerLine>,
}

#[derive(Resource, Default)]
pub struct Review {
    pub view: Option<ReviewView>,
}

#[derive(Resource, Default)]
pub struct PickState {
    /// Cursor position at left-press, to tell clicks from drags.
    pub press: Option<Vec2>,
    pub text: Option<String>,
}
