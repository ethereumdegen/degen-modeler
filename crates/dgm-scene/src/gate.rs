//! The deterministic gate's shared report type plus the budget rules that
//! fail ops closed. Mesh rules live in `dgm_mesh::validate`; UV rules in
//! `dgm-uv`; rig/anim rules in this crate. `dgm-jev::gate` composes them.

use dgm_atlas::Pack;
pub use dgm_mesh::{Finding, Severity};
use serde::Serialize;

use crate::doc::Doc;

#[derive(Debug, Clone, Serialize)]
pub struct GateReport {
    pub pass: bool,
    pub findings: Vec<Finding>,
}

impl GateReport {
    pub fn from_findings(findings: Vec<Finding>) -> Self {
        let pass = !findings.iter().any(|f| f.severity == Severity::Hard);
        Self { pass, findings }
    }
}

/// Budget rules (the law: an op that would breach them fails closed).
/// LODs are exempt from the class budget; they answer to their ratios.
pub fn budget_findings(doc: &Doc, pack: &Pack) -> Vec<Finding> {
    let mut findings = Vec::new();
    let Some(budget) = pack.budget(doc.asset_class) else {
        return findings;
    };
    for (name, object) in &doc.objects {
        if object.lod_of.is_some() {
            continue;
        }
        let tris = object.mesh.tri_count();
        if tris > budget.max_tris {
            findings.push(
                Finding::hard(
                    "budget.tris",
                    format!(
                        "object `{name}`: {tris} tris exceeds the {} budget of {}",
                        doc.asset_class, budget.max_tris
                    ),
                )
                .with_value(tris as f64),
            );
        }
    }
    findings
}
