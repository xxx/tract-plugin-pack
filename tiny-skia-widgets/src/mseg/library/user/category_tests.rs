use super::*;
use crate::mseg::{FactoryShape, factory_shape};
use std::sync::atomic::Ordering;

fn library() -> (CurveLibrary, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "tract-categories-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    (CurveLibrary::at(root.clone()), root)
}

#[test]
fn version_one_records_load_uncategorized_without_read_migration_or_legacy_wire_changes() {
    let (library, root) = library();
    fs::create_dir_all(&root).unwrap();
    let shape = factory_shape(FactoryShape::Duck);
    let bytes = serde_json::to_vec(&serde_json::json!({ "version": 1, "next_id": 2,
        "presets": [{ "id": 1, "name": "Legacy", "shape": shape }] }))
    .unwrap();
    fs::write(root.join("user-presets.json"), &bytes).unwrap();
    let legacy = library.user_presets().unwrap().remove(0);
    assert!(legacy.category.is_empty());
    assert_eq!(legacy.shape, shape);
    assert_eq!(fs::read(root.join("user-presets.json")).unwrap(), bytes);
    library.rename_user_preset(&legacy, "Renamed").unwrap();
    library.save_user_preset("Another", &shape).unwrap();
    let file: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("user-presets.json")).unwrap()).unwrap();
    assert_eq!(file["version"], 1);
    assert!(
        file["presets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|entry| entry.get("category").is_none())
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn categories_upgrade_atomically_and_legacy_rename_preserves_metadata_and_ids() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Triangle);
    let saved = library
        .save_user_preset_with_category("Pulse", "  Bass Δ  ", &shape)
        .unwrap();
    assert_eq!(saved.category, "Bass Δ");
    let renamed = library.rename_user_preset(&saved, "Renamed").unwrap();
    assert_eq!(renamed.id, saved.id);
    assert_eq!(renamed.category, saved.category);
    assert_eq!(renamed.shape, shape);
    let edited = library
        .update_user_preset(&renamed, "Renamed", "  ")
        .unwrap();
    assert_eq!(edited.id, saved.id);
    assert!(edited.category.is_empty());
    assert_eq!(edited.shape, shape);
    library.remove_user_preset(&edited).unwrap();
    let next = library.save_user_preset("New", &shape).unwrap();
    assert!(next.id > saved.id);
    let file: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("user-presets.json")).unwrap()).unwrap();
    assert_eq!(file["version"], 2);
    assert_eq!(library.user_presets().unwrap(), [next]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_categories_and_cross_category_name_duplicates_preserve_bytes() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Sine);
    let saved = library
        .save_user_preset_with_category("Pulse", "Bass", &shape)
        .unwrap();
    let before = fs::read(root.join("user-presets.json")).unwrap();
    for category in ["x\ny".to_owned(), "x\0y".to_owned(), "曲".repeat(65)] {
        assert!(
            library
                .save_user_preset_with_category("Another", &category, &shape)
                .is_err()
        );
        assert!(
            library
                .update_user_preset(&saved, "Renamed", &category)
                .is_err()
        );
    }
    assert!(
        library
            .save_user_preset_with_category("PULSE", "Other", &shape)
            .is_err()
    );
    assert_eq!(fs::read(root.join("user-presets.json")).unwrap(), before);
    let max = library
        .update_user_preset(&saved, "Pulse", &"曲".repeat(64))
        .unwrap();
    assert_eq!(max.category.chars().count(), 64);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn category_changes_make_old_full_record_actions_stale_and_preserve_unrelated_writes() {
    let (first, root) = library();
    let second = CurveLibrary::at(root.clone());
    let shape = factory_shape(FactoryShape::Duck);
    let original = first
        .save_user_preset_with_category("Original", "Bass", &shape)
        .unwrap();
    let other = second
        .save_user_preset_with_category("Other", "Lead", &shape)
        .unwrap();
    let changed = second
        .update_user_preset(&original, "Original", "Reverb")
        .unwrap();
    assert!(first.load_user_preset(&original).is_err());
    assert!(first.rename_user_preset(&original, "Stale").is_err());
    assert!(
        first
            .update_user_preset(&original, "Stale", "Wrong")
            .is_err()
    );
    assert!(first.remove_user_preset(&original).is_err());
    assert_eq!(first.user_presets().unwrap(), [changed, other]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn invalid_category_versions_and_records_are_not_silently_replaced() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Duck);
    let saved = library
        .save_user_preset_with_category("Saved", "Bass", &shape)
        .unwrap();
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("user-presets.json")).unwrap()).unwrap();
    for case in 0..6 {
        let mut bank = valid.clone();
        match case {
            0 => bank["version"] = 1.into(),
            1 => bank["version"] = 3.into(),
            2 => bank["presets"][0]["category"] = " untrimmed ".into(),
            3 => bank["presets"][0]["category"] = "x\ny".into(),
            4 => bank["presets"][0]["category"] = "x".repeat(65).into(),
            _ => bank["presets"][0]["category"] = serde_json::Value::Null,
        }
        let bytes = serde_json::to_vec(&bank).unwrap();
        fs::write(root.join("user-presets.json"), &bytes).unwrap();
        assert!(library.user_presets().is_err());
        assert!(library.save_user_preset("New", &shape).is_err());
        assert!(
            library
                .update_user_preset(&saved, "Renamed", "New")
                .is_err()
        );
        assert!(library.remove_user_preset(&saved).is_err());
        assert_eq!(fs::read(root.join("user-presets.json")).unwrap(), bytes);
    }
    fs::remove_dir_all(root).unwrap();
}
