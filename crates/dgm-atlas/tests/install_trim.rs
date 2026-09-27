use std::path::{Path, PathBuf};

use dgm_atlas::{Pack, install_trim};

fn classic() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../packs/classic")
}

/// A private copy of the classic pack: install writes into the pack dir.
fn scratch_pack(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("dgm-trim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("trims")).unwrap();
    std::fs::copy(classic().join("pack.json"), dir.join("pack.json")).unwrap();
    for sheet in ["wood", "stone", "metal", "cloth", "foliage"] {
        let f = format!("trims/{sheet}.png");
        std::fs::copy(classic().join(&f), dir.join(&f)).unwrap();
    }
    dir
}

#[test]
fn installed_png_becomes_a_full_sheet_trim_and_keeps_the_pack_loadable() {
    let pack = scratch_pack("ok");
    let size = install_trim(&pack, "cave_rock", "rock", &classic().join("trims/stone.png"), false)
        .unwrap();
    assert_eq!(size, [512, 512]);

    let p = Pack::load(&pack).unwrap();
    assert_eq!(p.region_uv("cave_rock", "rock"), Some([0.0, 0.0, 1.0, 1.0]));
    assert!(p.texture_path("cave_rock").unwrap().exists());
    // Everything that was there before survives the JSON edit.
    assert!(p.manifest.trims.contains_key("wood"));
    assert!(p.manifest.rigs.contains_key("biped"));
    std::fs::remove_dir_all(&pack).ok();
}

#[test]
fn an_existing_sheet_is_refused_unless_forced() {
    let pack = scratch_pack("dup");
    let src = classic().join("trims/wood.png");
    let err = install_trim(&pack, "wood", "planks", &src, false).unwrap_err().to_string();
    assert!(err.contains("already exists") && err.contains("--force"), "{err}");
    install_trim(&pack, "wood", "planks", &src, true).unwrap();
    std::fs::remove_dir_all(&pack).ok();
}

#[test]
fn non_png_and_unsafe_names_are_refused_before_anything_is_written() {
    let pack = scratch_pack("bad");
    let before = std::fs::read(pack.join("pack.json")).unwrap();

    let err = install_trim(&pack, "x", "main", &pack.join("pack.json"), false).unwrap_err();
    assert!(err.to_string().contains("not a PNG"), "{err}");
    let err = install_trim(&pack, "../escape", "main", &classic().join("trims/wood.png"), false)
        .unwrap_err();
    assert!(err.to_string().contains("names must be"), "{err}");

    assert_eq!(std::fs::read(pack.join("pack.json")).unwrap(), before, "manifest untouched");
    std::fs::remove_dir_all(&pack).ok();
}
