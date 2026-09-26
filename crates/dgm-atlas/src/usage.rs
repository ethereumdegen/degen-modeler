//! Region usage index: which objects use which trim regions.
//!
//! `dgm-atlas` sits below `dgm-scene`, so this takes plain inputs instead
//! of a `Doc`. Integration adapts: for every object whose material is
//! `Trim { sheet }`, pass `(sheet name, one representative UV point per
//! face — the face's UV centroid)`.

use std::collections::BTreeMap;

use crate::pack::Pack;

const EPS: f32 = 1e-5;

/// Classify per-object UV points into the trim regions of their sheet.
///
/// Returns `"sheet/region" -> [object, ...]` with objects in name order,
/// each listed once per region it touches. Points on a region border count
/// as inside (a point on the boundary of two regions credits both); objects
/// naming an unknown sheet are skipped.
pub fn region_usage(
    uv_points: &BTreeMap<String, (String, Vec<[f32; 2]>)>,
    pack: &Pack,
) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (object, (sheet, points)) in uv_points {
        let Some(trim) = pack.manifest.trims.get(sheet) else { continue };
        for region in trim.regions.keys() {
            let Some([u0, v0, u1, v1]) = pack.region_uv(sheet, region) else { continue };
            let used = points.iter().any(|&[u, v]| {
                u >= u0 - EPS && u <= u1 + EPS && v >= v0 - EPS && v <= v1 + EPS
            });
            if used {
                out.entry(format!("{sheet}/{region}")).or_default().push(object.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    use crate::pack::{Band, Pack, PackManifest, Rect, TrimSheet};

    use super::region_usage;

    fn pack() -> Pack {
        let mut regions = BTreeMap::new();
        regions.insert("planks".to_string(), Rect { x: 0, y: 0, w: 128, h: 64 });
        regions.insert("stone".to_string(), Rect { x: 0, y: 64, w: 128, h: 64 });
        let mut trims = BTreeMap::new();
        trims.insert(
            "main".to_string(),
            TrimSheet { file: "main.png".into(), size: [128, 128], regions },
        );
        Pack {
            root: PathBuf::from("."),
            manifest: PackManifest {
                name: "t".into(),
                version: "0".into(),
                budgets: BTreeMap::new(),
                texel_density: Band { min: 32.0, max: 512.0 },
                texture_sizes: vec![128],
                palette: vec![],
                hard_edge_angle_deg: 40.0,
                uv_waste_max: 0.15,
                trims,
                rigs: BTreeMap::new(),
                clips: BTreeMap::new(),
                reference_board: None,
            },
        }
    }

    #[test]
    fn classifies_points_into_regions() {
        let pack = pack();
        let mut uv_points = BTreeMap::new();
        // hut: only in planks (top half); cart: both regions.
        uv_points.insert("hut".to_string(), ("main".to_string(), vec![[0.5, 0.25]]));
        uv_points
            .insert("cart".to_string(), ("main".to_string(), vec![[0.1, 0.1], [0.9, 0.9]]));
        // ghost: unknown sheet -> ignored.
        uv_points.insert("ghost".to_string(), ("nope".to_string(), vec![[0.5, 0.5]]));

        let usage = region_usage(&uv_points, &pack);
        assert_eq!(
            usage.get("main/planks"),
            Some(&vec!["cart".to_string(), "hut".to_string()])
        );
        assert_eq!(usage.get("main/stone"), Some(&vec!["cart".to_string()]));
        assert_eq!(usage.len(), 2);
    }

    #[test]
    fn border_point_credits_touching_regions() {
        let pack = pack();
        let mut uv_points = BTreeMap::new();
        // v = 0.5 is the planks/stone boundary.
        uv_points.insert("post".to_string(), ("main".to_string(), vec![[0.5, 0.5]]));
        let usage = region_usage(&uv_points, &pack);
        assert_eq!(usage.get("main/planks"), Some(&vec!["post".to_string()]));
        assert_eq!(usage.get("main/stone"), Some(&vec!["post".to_string()]));
    }
}
