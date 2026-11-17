use super::*;
use crate::mseg::library::{CurveLibrary, NEXT_WRITE};
use crate::mseg::{FactoryShape, factory_shape};
use std::fs;
use std::sync::atomic::Ordering;

fn library() -> (CurveLibrary, std::path::PathBuf) {
    let root = std::env::temp_dir().join(format!(
        "tract-user-presets-{}-{}",
        std::process::id(),
        NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
    ));
    (CurveLibrary::at(root.clone()), root)
}

#[test]
fn named_presets_share_round_trip_identity_and_preserve_legacy_files() {
    let (first, root) = library();
    let second = CurveLibrary::at(root.clone());
    assert!(first.user_presets().unwrap().is_empty());
    assert!(!root.exists());
    let duck = factory_shape(FactoryShape::Duck);
    let square = factory_shape(FactoryShape::Square);
    first.copy(&duck).unwrap();
    first.store_favorite(3, &square).unwrap();
    let saved = first.save_user_preset("  減衰 Δ  ", &duck).unwrap();
    assert_eq!(saved.name, "減衰 Δ");
    assert!(saved.id > 0);
    assert_eq!(second.user_presets().unwrap(), [saved.clone()]);
    let renamed = second.rename_user_preset(&saved, "Soft duck").unwrap();
    assert_eq!(renamed.id, saved.id);
    assert_eq!(renamed.shape, duck);
    assert_eq!(first.load_user_preset(&renamed).unwrap(), duck);
    second.remove_user_preset(&renamed).unwrap();
    assert!(first.user_presets().unwrap().is_empty());
    let next = first.save_user_preset("Another", &square).unwrap();
    assert!(next.id > saved.id);
    assert_eq!(second.paste().unwrap(), duck);
    assert_eq!(second.load_favorite(3).unwrap(), square);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn duplicate_and_invalid_names_are_transactional_and_listing_is_sorted() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Sine);
    let z = library.save_user_preset("Zulu", &shape).unwrap();
    let a = library.save_user_preset("alpha", &shape).unwrap();
    let before = fs::read(root.join("user-presets.json")).unwrap();
    for name in [
        "",
        "  ",
        "a\nb",
        "a\0b",
        &"x".repeat(MAX_USER_PRESET_NAME_CHARS + 1),
    ] {
        assert!(library.save_user_preset(name, &shape).is_err());
        assert!(library.rename_user_preset(&z, name).is_err());
    }
    for name in ["ALPHA", " alpha ", "Alpha"] {
        assert!(library.save_user_preset(name, &shape).is_err());
        assert!(library.rename_user_preset(&z, name).is_err());
    }
    assert_eq!(fs::read(root.join("user-presets.json")).unwrap(), before);
    assert_eq!(library.user_presets().unwrap(), [a, z]);
    let name = "曲".repeat(MAX_USER_PRESET_NAME_CHARS);
    assert_eq!(library.save_user_preset(&name, &shape).unwrap().name, name);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn stale_record_actions_preserve_unrelated_cross_instance_updates() {
    let (first, root) = library();
    let second = CurveLibrary::at(root.clone());
    let shape = factory_shape(FactoryShape::Duck);
    let original = first.save_user_preset("Original", &shape).unwrap();
    let other = second.save_user_preset("Unrelated", &shape).unwrap();
    let renamed = first.rename_user_preset(&original, "Renamed").unwrap();
    assert!(second.load_user_preset(&original).is_err());
    assert!(second.rename_user_preset(&original, "Stale").is_err());
    assert!(second.remove_user_preset(&original).is_err());
    assert_eq!(
        second.user_presets().unwrap(),
        [renamed.clone(), other.clone()]
    );
    first.remove_user_preset(&renamed).unwrap();
    let new = first.save_user_preset("Original", &shape).unwrap();
    assert_ne!(new.id, original.id);
    assert!(second.remove_user_preset(&original).is_err());
    assert_eq!(second.user_presets().unwrap(), [new, other]);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn malformed_banks_are_reported_without_erasing_existing_bytes() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Triangle);
    let saved = library.save_user_preset("Saved", &shape).unwrap();
    let valid: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("user-presets.json")).unwrap()).unwrap();
    let mut cases = vec!["bad json".to_owned(), "x".repeat(MAX_USER_PRESET_BYTES + 1)];
    for mutation in 0..5 {
        let mut bank = valid.clone();
        match mutation {
            0 => bank["version"] = 999.into(),
            1 => bank["presets"][0]["id"] = 0.into(),
            2 => bank["next_id"] = saved.id.into(),
            3 => {
                let duplicate = bank["presets"][0].clone();
                bank["presets"].as_array_mut().unwrap().push(duplicate);
            }
            _ => bank["presets"][0]["shape"]["nodes"][0]["time"] = 0.1.into(),
        }
        cases.push(serde_json::to_string(&bank).unwrap());
    }
    for text in cases {
        fs::write(root.join("user-presets.json"), &text).unwrap();
        assert!(library.user_presets().is_err());
        assert!(library.save_user_preset("New", &shape).is_err());
        assert!(library.rename_user_preset(&saved, "New").is_err());
        assert!(library.remove_user_preset(&saved).is_err());
        assert_eq!(
            fs::read_to_string(root.join("user-presets.json")).unwrap(),
            text
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn busy_library_rejects_a_write_and_recovers_after_unlock() {
    let (library, root) = library();
    let shape = factory_shape(FactoryShape::Duck);
    let saved = library.save_user_preset("Saved", &shape).unwrap();
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("user-presets.lock"))
        .unwrap();
    lock.try_lock().unwrap();
    assert!(library.save_user_preset("Blocked", &shape).is_err());
    assert!(library.rename_user_preset(&saved, "Blocked").is_err());
    assert!(library.remove_user_preset(&saved).is_err());
    assert_eq!(library.load_user_preset(&saved).unwrap(), shape);
    drop(lock);
    library.rename_user_preset(&saved, "Renamed").unwrap();
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn full_library_allows_rename_delete_and_reuses_capacity_without_reusing_identity() {
    let (library, root) = library();
    fs::create_dir_all(&root).unwrap();
    let shape = factory_shape(FactoryShape::Triangle);
    let bank = UserBank {
        next_id: MAX_USER_PRESETS as u64 + 1,
        presets: (1..=MAX_USER_PRESETS)
            .map(|id| UserPreset {
                id: id as u64,
                name: format!("Curve {id:03}"),
                category: String::new(),
                shape: shape.clone(),
            })
            .collect(),
        ..UserBank::default()
    };
    fs::write(
        root.join("user-presets.json"),
        serde_json::to_string(&bank).unwrap(),
    )
    .unwrap();
    let before = fs::read(root.join("user-presets.json")).unwrap();
    assert!(library.save_user_preset("Over capacity", &shape).is_err());
    assert_eq!(fs::read(root.join("user-presets.json")).unwrap(), before);
    let renamed = library
        .rename_user_preset(&bank.presets[0], "Renamed")
        .unwrap();
    library.remove_user_preset(&renamed).unwrap();
    let saved = library.save_user_preset("New", &shape).unwrap();
    assert_eq!(saved.id, MAX_USER_PRESETS as u64 + 1);
    assert_eq!(library.user_presets().unwrap().len(), MAX_USER_PRESETS);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn concurrent_library_writers_retain_every_successful_save() {
    let (library, root) = library();
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(4));
    std::thread::scope(|scope| {
        for worker in 0..4 {
            let root = root.clone();
            let barrier = barrier.clone();
            scope.spawn(move || {
                let library = CurveLibrary::at(root);
                let shape = factory_shape(FactoryShape::Duck);
                barrier.wait();
                for index in 0..4 {
                    let start = std::time::Instant::now();
                    loop {
                        match library
                            .save_user_preset(&format!("Worker {worker} / {index}"), &shape)
                        {
                            Ok(_) => break,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                assert!(
                                    start.elapsed().as_secs() < 5,
                                    "writer did not acquire the lock"
                                );
                                std::thread::yield_now();
                            }
                            Err(error) => panic!("save failed: {error}"),
                        }
                    }
                }
            });
        }
    });
    let presets = library.user_presets().unwrap();
    assert_eq!(presets.len(), 16);
    assert_eq!(
        presets
            .iter()
            .map(|preset| preset.id)
            .collect::<std::collections::HashSet<_>>()
            .len(),
        16
    );
    fs::remove_dir_all(root).unwrap();
}
