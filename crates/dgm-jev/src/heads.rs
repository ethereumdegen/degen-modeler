//! Tier 1: fixed-formula heads over the `dgm_render::metrics` JSON, the
//! scene digest JSON, and the gate report. All heads are 0..1 with 1 = good.
//! Missing or `null` metrics score neutral (1.0 for their component) so the
//! formulas degrade instead of erroring.

use std::collections::BTreeMap;

use dgm_scene::GateReport;
use serde_json::Value;

/// `seams = 1 - clamp01(seam_contrast_max * SEAM_GAIN)`: a max cross-seam
/// colour delta of `1/SEAM_GAIN` (0.5) already zeroes the head.
pub const SEAM_GAIN: f32 = 2.0;
/// `uv_occupancy` at or above this fraction of `[0,1]^2` scores full marks.
pub const OCCUPANCY_TARGET: f32 = 0.85;
/// `density_spread` (max/min face texel density) of 1 is perfect; the fit
/// component reaches 0 at this spread.
pub const DENSITY_SPREAD_MAX: f32 = 4.0;
/// Silhouette penalty gain on LOD `mask_drift` (a drift of 0.25 zeroes it).
pub const MASK_DRIFT_GAIN: f32 = 4.0;
/// Style penalty gain on mean `palette_distance` (0.5 zeroes it).
pub const PALETTE_GAIN: f32 = 2.0;
/// Style penalty per `uv.texel_band` gate finding.
pub const TEXEL_BAND_PENALTY: f32 = 0.25;

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn metric(metrics: &Value, key: &str) -> Option<f32> {
    metrics.get(key)?.as_f64().map(|f| f as f32)
}

/// The documented formulas (constants above):
/// - `seams`      = 1 - clamp01(seam_contrast_max * SEAM_GAIN)
/// - `waste`      = mean(occupancy_fit, spread_fit, budget_fit) where
///   occupancy_fit = clamp01(uv_occupancy / OCCUPANCY_TARGET),
///   spread_fit    = clamp01(1 - (density_spread - 1) / (DENSITY_SPREAD_MAX - 1)),
///   budget_fit    = 1 while `digest.budget.used_tris <= max_tris`,
///                   else max_tris / used_tris
/// - `silhouette` = mean(1 - clamp01(mask_drift * MASK_DRIFT_GAIN), spread_fit)
/// - `style`      = mean(1 - clamp01(palette_distance * PALETTE_GAIN),
///                  clamp01(1 - TEXEL_BAND_PENALTY * count(`uv.texel_band`)))
/// - `done`       = min(all above) * (1 if the gate passes, else 0)
pub fn formula_heads(gate: &GateReport, metrics: &Value, digest: &Value) -> BTreeMap<String, f32> {
    let seams = 1.0 - clamp01(metric(metrics, "seam_contrast_max").unwrap_or(0.0) * SEAM_GAIN);

    let spread = metric(metrics, "density_spread").unwrap_or(1.0).max(1.0);
    let spread_fit = clamp01(1.0 - (spread - 1.0) / (DENSITY_SPREAD_MAX - 1.0));

    let occupancy_fit = match metric(metrics, "uv_occupancy") {
        Some(occ) => clamp01(occ / OCCUPANCY_TARGET),
        None => 1.0, // all-trim materials: nothing owned to waste
    };
    let budget_fit = digest
        .get("budget")
        .and_then(|b| {
            let used = b.get("used_tris")?.as_f64()? as f32;
            let max = b.get("max_tris")?.as_f64()? as f32;
            Some(if used <= max || used <= 0.0 { 1.0 } else { clamp01(max / used) })
        })
        .unwrap_or(1.0);
    let waste = (occupancy_fit + spread_fit + budget_fit) / 3.0;

    let drift_fit = match metric(metrics, "mask_drift") {
        Some(drift) => 1.0 - clamp01(drift * MASK_DRIFT_GAIN),
        None => 1.0, // no LODs: nothing to drift
    };
    let silhouette = (drift_fit + spread_fit) / 2.0;

    let palette_fit = 1.0 - clamp01(metric(metrics, "palette_distance").unwrap_or(0.0) * PALETTE_GAIN);
    let band_hits = gate.findings.iter().filter(|f| f.rule == "uv.texel_band").count() as f32;
    let band_fit = clamp01(1.0 - band_hits * TEXEL_BAND_PENALTY);
    let style = (palette_fit + band_fit) / 2.0;

    let done =
        silhouette.min(style).min(seams).min(waste) * if gate.pass { 1.0 } else { 0.0 };

    BTreeMap::from([
        ("done".to_string(), done),
        ("seams".to_string(), seams),
        ("silhouette".to_string(), silhouette),
        ("style".to_string(), style),
        ("waste".to_string(), waste),
    ])
}

#[cfg(test)]
mod tests {
    use dgm_mesh::Finding;
    use serde_json::json;

    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn formula_heads_from_hand_built_metrics() {
        let gate = GateReport::from_findings(vec![]);
        let metrics = json!({
            "seam_contrast_max": 0.1, "seam_contrast_mean": 0.05,
            "uv_occupancy": 0.68, "stretch_max": 1.5, "stretch_mean": 1.1,
            "palette_distance": 0.2, "density_spread": 2.5, "mask_drift": 0.05,
            "per_object": {}
        });
        let digest = json!({"budget": {"class": "prop", "max_tris": 800, "used_tris": 400}});
        let heads = formula_heads(&gate, &metrics, &digest);

        assert!(close(heads["seams"], 0.8)); // 1 - 0.1*2
        // spread_fit = 1 - 1.5/3 = 0.5; occ = 0.68/0.85 = 0.8; budget = 1
        assert!(close(heads["waste"], (0.8 + 0.5 + 1.0) / 3.0));
        assert!(close(heads["silhouette"], (0.8 + 0.5) / 2.0)); // drift 0.05*4
        assert!(close(heads["style"], (0.6 + 1.0) / 2.0)); // palette 0.2*2
        assert!(close(heads["done"], heads["silhouette"])); // the min, gate passes
    }

    #[test]
    fn nulls_score_neutral_and_hard_gate_zeroes_done() {
        let gate = GateReport::from_findings(vec![
            Finding::hard("mesh.degenerate_face", "face f0 has ~zero area"),
            Finding::hard("uv.texel_band", "face f1 outside band"),
        ]);
        let metrics = json!({"uv_occupancy": null, "mask_drift": null, "per_object": {}});
        let heads = formula_heads(&gate, &metrics, &json!({}));

        assert!(close(heads["seams"], 1.0));
        assert!(close(heads["waste"], 1.0));
        assert!(close(heads["silhouette"], 1.0));
        assert!(close(heads["style"], (1.0 + 0.75) / 2.0)); // one texel_band hit
        assert!(close(heads["done"], 0.0)); // gate fails
    }

    #[test]
    fn over_budget_shrinks_waste() {
        let gate = GateReport::from_findings(vec![]);
        let digest = json!({"budget": {"class": "prop", "max_tris": 500, "used_tris": 1000}});
        let heads = formula_heads(&gate, &json!({"per_object": {}}), &digest);
        // occ neutral 1, spread neutral 1, budget_fit 0.5
        assert!(close(heads["waste"], (1.0 + 1.0 + 0.5) / 3.0));
    }
}
