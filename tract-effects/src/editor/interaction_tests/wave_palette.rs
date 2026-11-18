use super::*;
use std::sync::{Arc, Mutex};
use widgets::mseg::{CurveShape, FactoryShape, MsegEdit, WavePalette, factory_shape};

fn project(h: &mut Harness) {
    assert!(
        h.editor
            .mseg_edit
            .open_curve_browser(h.geometry.layout.mseg_pane, h.geometry.scale)
    );
    assert!(!h.key(Key::Tab).changed);
    assert!(!h.key(Key::Tab).changed);
}

#[test]
fn project_store_clear_and_refresh_do_not_report_audio_or_curve_edits() {
    let mut h = Harness::new();
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    h.editor.bind_wave_palette(palette.clone());
    h.editor.set_audio_view(AudioView::Both);
    h.editor.set_audio_frozen(true);
    let before = (h.effect, h.modulation.clone());
    let slot = h.editor.selected_mseg;
    project(&mut h);
    for value in ["s", "s", "r"] {
        let response = h.key(Key::Character(value.into()));
        assert!(response.captured && !response.changed);
        assert_eq!(
            palette.lock().unwrap().get(0),
            Some(&CurveShape::capture(&before.1.msegs[slot]))
        );
        assert_eq!((h.effect, h.modulation.clone()), before);
    }
    let response = h.key(Key::Delete);
    assert!(response.captured && !response.changed);
    assert!(palette.lock().unwrap().get(0).is_none());
    assert_eq!((h.effect, h.modulation), before);
    assert!(h.editor.audio_frozen());
}

#[test]
fn project_recall_reports_one_selected_curve_change_and_preserves_the_bank() {
    let mut h = Harness::new();
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    let shape = factory_shape(FactoryShape::Duck);
    palette.lock().unwrap().store(0, &shape).unwrap();
    h.editor.bind_wave_palette(palette.clone());
    let before = (h.effect, h.modulation.clone());
    let mut expected = h.modulation.clone();
    shape
        .apply(&mut expected.msegs[h.editor.selected_mseg], false)
        .unwrap();
    project(&mut h);
    let response = h.key(Key::Enter);
    assert!(response.captured && response.changed);
    assert_eq!(h.effect, before.0);
    assert_eq!(h.modulation, expected);
    assert!(!h.release().changed);
    project(&mut h);
    let response = h.key(Key::Enter);
    assert!(response.captured && !response.changed);
    assert_eq!(palette.lock().unwrap().get(0), Some(&shape));
}

#[test]
fn palette_binding_survives_shared_editor_cancellation_and_other_curve_selection() {
    let mut h = Harness::new();
    let palette = Arc::new(Mutex::new(WavePalette::default()));
    palette
        .lock()
        .unwrap()
        .store(0, &factory_shape(FactoryShape::Triangle))
        .unwrap();
    h.editor.bind_wave_palette(palette.clone());
    project(&mut h);
    h.editor.cancel_interaction();
    assert!(!h.editor.dropdown_open());
    assert!(Arc::ptr_eq(
        h.editor.mseg_edit.wave_palette().unwrap(),
        &palette
    ));
    h.editor.selected_mseg = 3;
    project(&mut h);
    let rect = h.geometry.layout.mseg_pane;
    assert_eq!(
        h.editor.mseg_edit.on_key(
            &KeyboardEvent {
                key: Key::Enter,
                state: KeyState::Down,
                ..KeyboardEvent::default()
            },
            &mut h.modulation.msegs[3],
            rect,
            h.geometry.scale
        ),
        (true, Some(MsegEdit::Changed))
    );
    assert_eq!(
        h.modulation.msegs[3].active(),
        factory_shape(FactoryShape::Triangle).nodes()
    );
    assert_eq!(
        palette.lock().unwrap().get(0),
        Some(&factory_shape(FactoryShape::Triangle))
    );
}
