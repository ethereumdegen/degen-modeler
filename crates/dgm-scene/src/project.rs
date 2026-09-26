//! A project on disk: `project.json`, the pack copy, `ops.jsonl`, artifacts.
//!
//! ```text
//! <root>/project.json   { name, class, goal, pack: "pack" }
//! <root>/pack/          self-contained copy of the style pack
//! <root>/ops.jsonl      the ledger
//! <root>/artifacts/     renders + review reports
//! <root>/exports/       .glb outputs
//! ```

use std::path::{Path, PathBuf};

use chrono::SecondsFormat;
use dgm_atlas::{AssetClass, Pack, PackError};
use serde::{Deserialize, Serialize};

use crate::dispatch;
use crate::doc::Doc;
use crate::error::OpError;
use crate::gate;
use crate::ledger::{self, LedgerError, LedgerLine};
use crate::op::{Op, Outcome};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectMeta {
    pub name: String,
    pub class: AssetClass,
    #[serde(default)]
    pub goal: Option<String>,
    /// Pack dir, relative to the project root.
    pub pack: String,
}

#[derive(Debug, thiserror::Error)]
pub enum ProjectError {
    #[error("project {0}: {1}")]
    Io(PathBuf, std::io::Error),
    #[error("project {0}: {1}")]
    Meta(PathBuf, serde_json::Error),
    #[error(transparent)]
    Pack(#[from] PackError),
    #[error(transparent)]
    Ledger(#[from] LedgerError),
    #[error("replaying rev {rev}: {source}")]
    Replay { rev: u64, source: OpError },
}

pub struct Project {
    pub root: PathBuf,
    pub meta: ProjectMeta,
    pub pack: Pack,
    pub doc: Doc,
}

impl Project {
    /// Create a fresh project, copying the pack in so it is self-contained.
    pub fn init(
        root: &Path,
        pack_src: &Path,
        name: &str,
        class: AssetClass,
        goal: Option<String>,
    ) -> Result<Self, ProjectError> {
        let io = |e| ProjectError::Io(root.to_path_buf(), e);
        std::fs::create_dir_all(root).map_err(io)?;
        std::fs::create_dir_all(root.join("artifacts")).map_err(io)?;
        std::fs::create_dir_all(root.join("exports")).map_err(io)?;
        copy_dir(pack_src, &root.join("pack")).map_err(io)?;
        let meta = ProjectMeta {
            name: name.into(),
            class,
            goal: goal.clone(),
            pack: "pack".into(),
        };
        std::fs::write(
            root.join("project.json"),
            serde_json::to_vec_pretty(&meta).expect("meta serializes"),
        )
        .map_err(io)?;
        std::fs::write(root.join("ops.jsonl"), b"").map_err(io)?;
        Self::load(root)
    }

    pub fn load(root: &Path) -> Result<Self, ProjectError> {
        let meta_path = root.join("project.json");
        let text = std::fs::read_to_string(&meta_path)
            .map_err(|e| ProjectError::Io(meta_path.clone(), e))?;
        let meta: ProjectMeta =
            serde_json::from_str(&text).map_err(|e| ProjectError::Meta(meta_path, e))?;
        let pack = Pack::load(&root.join(&meta.pack))?;
        let doc = replay(root, &meta, &pack)?;
        Ok(Self { root: root.to_path_buf(), meta, pack, doc })
    }

    pub fn ledger_path(&self) -> PathBuf {
        self.root.join("ops.jsonl")
    }

    pub fn artifacts_dir(&self) -> PathBuf {
        self.root.join("artifacts")
    }

    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    /// Apply one op: validate on a scratch doc, fail closed on hard budget
    /// findings, append to the ledger, commit, bump the revision.
    pub fn apply(&mut self, op: Op, actor: &str) -> Result<Outcome, OpError> {
        let mut next = self.doc.clone();
        let diff = dispatch::apply(&mut next, &self.pack, &op)?;

        let findings = gate::budget_findings(&next, &self.pack);
        if findings.iter().any(|f| f.severity == dgm_mesh::Severity::Hard) {
            let message = findings
                .iter()
                .map(|f| f.message.as_str())
                .collect::<Vec<_>>()
                .join("; ");
            return Err(OpError::Budget { message, findings });
        }

        next.revision = self.doc.revision + 1;
        ledger::append(
            &self.ledger_path(),
            &LedgerLine {
                rev: next.revision,
                parent: self.doc.revision,
                actor: actor.into(),
                time: chrono::Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
                op,
            },
        )?;
        self.doc = next;
        Ok(Outcome { revision: self.doc.revision, diff, findings })
    }

    /// Law check: replay the ledger from scratch; the result must equal the
    /// live doc (callers compare exports byte-for-byte).
    pub fn replay_fresh(&self) -> Result<Doc, ProjectError> {
        replay(&self.root, &self.meta, &self.pack)
    }
}

fn replay(root: &Path, meta: &ProjectMeta, pack: &Pack) -> Result<Doc, ProjectError> {
    let mut doc = Doc::new(meta.class);
    doc.goal = meta.goal.clone();
    for line in ledger::read(&root.join("ops.jsonl"))? {
        dispatch::apply(&mut doc, pack, &line.op)
            .map_err(|source| ProjectError::Replay { rev: line.rev, source })?;
        doc.revision = line.rev;
    }
    Ok(doc)
}

fn copy_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
