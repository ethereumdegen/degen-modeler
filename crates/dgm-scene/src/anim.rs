use std::collections::BTreeMap;

use dgm_mesh::Finding;
use serde::{Deserialize, Serialize};

use crate::doc::Doc;

/// Keyframed curves for one bone. Rotations are quaternions (x, y, z, w).
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct Channel {
    pub times: Vec<f32>,
    #[serde(default)]
    pub rotations: Option<Vec<[f32; 4]>>,
    #[serde(default)]
    pub translations: Option<Vec<[f32; 3]>>,
    #[serde(default)]
    pub scales: Option<Vec<[f32; 3]>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Clip {
    pub object: String,
    pub looped: bool,
    pub duration: f32,
    /// Bone name -> curves.
    pub channels: BTreeMap<String, Channel>,
}

/// Animation gate rules: channels name rig bones, times sorted and in range,
/// curve lengths match their key times.
pub fn clip_findings(doc: &Doc) -> Vec<Finding> {
    let mut findings = Vec::new();
    for (clip_name, clip) in &doc.clips {
        let Ok(object) = doc.object(&clip.object) else {
            findings.push(Finding::hard(
                "anim.orphan_clip",
                format!("clip `{clip_name}` targets missing object `{}`", clip.object),
            ));
            continue;
        };
        let Some(rig) = &object.rig else {
            findings.push(Finding::hard(
                "anim.unrigged_object",
                format!("clip `{clip_name}` targets unrigged object `{}`", clip.object),
            ));
            continue;
        };
        for (bone, ch) in &clip.channels {
            if rig.bone_index(bone).is_none() {
                findings.push(Finding::hard(
                    "anim.unknown_bone",
                    format!("clip `{clip_name}` keys unknown bone `{bone}`"),
                ));
            }
            if ch.times.windows(2).any(|w| w[0] >= w[1]) {
                findings.push(Finding::hard(
                    "anim.unsorted_times",
                    format!("clip `{clip_name}` bone `{bone}`: key times not strictly increasing"),
                ));
            }
            if ch.times.iter().any(|&t| t < 0.0 || t > clip.duration + 1e-4) {
                findings.push(Finding::hard(
                    "anim.time_out_of_range",
                    format!("clip `{clip_name}` bone `{bone}`: key outside 0..{}", clip.duration),
                ));
            }
            for (kind, len) in [
                ("rotations", ch.rotations.as_ref().map(Vec::len)),
                ("translations", ch.translations.as_ref().map(Vec::len)),
                ("scales", ch.scales.as_ref().map(Vec::len)),
            ] {
                if let Some(len) = len
                    && len != ch.times.len()
                {
                    findings.push(Finding::hard(
                        "anim.curve_length",
                        format!(
                            "clip `{clip_name}` bone `{bone}`: {kind} has {len} keys for {} times",
                            ch.times.len()
                        ),
                    ));
                }
            }
        }
    }
    findings
}
