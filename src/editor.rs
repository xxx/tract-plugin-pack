mod filter_response_view;
mod param_dial;
mod wavetable_view;

use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::WavetableFilterParams;
use filter_response_view::FilterResponseView;
use param_dial::ParamDial;
use wavetable_view::WavetableView;

const SCALE_STEPS: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0];

/// Find the closest SCALE_STEPS index for a given scale factor.
fn nearest_scale_idx(scale: f64) -> usize {
    SCALE_STEPS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            (*a - scale).abs().partial_cmp(&(*b - scale).abs()).unwrap()
        })
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Set the ui_scale IntParam via the nih-plug parameter event system.
fn set_scale_param(cx: &mut EventContext, param: &nih_plug::prelude::IntParam, scale: f64) {
    use nih_plug::prelude::Param;
    let pct = (scale * 100.0).round() as i32;
    let normalized = param.preview_normalized(pct);
    cx.emit(RawParamEvent::BeginSetParameter(param.as_ptr()));
    cx.emit(RawParamEvent::SetParameterNormalized(
        param.as_ptr(),
        normalized,
    ));
    cx.emit(RawParamEvent::EndSetParameter(param.as_ptr()));
}


#[derive(Lens, Clone)]
struct Data {
    params: Arc<WavetableFilterParams>,
    wavetable_path: String,
    wavetable_version: u32,
    status_message: String,
    ui_scale_pct: String,
}

enum DataEvent {
    SetWavetablePath(String),
    WavetableChanged(u32),
    SetStatus(String),
    SetUiScalePct(String),
}

impl Model for Data {
    fn event(&mut self, _cx: &mut EventContext, event: &mut Event) {
        event.map(|data_event, _| match data_event {
            DataEvent::SetWavetablePath(path) => {
                self.wavetable_path = path.clone();
            }
            DataEvent::WavetableChanged(version) => {
                self.wavetable_version = *version;
            }
            DataEvent::SetStatus(msg) => {
                self.status_message = msg.clone();
            }
            DataEvent::SetUiScalePct(pct) => {
                self.ui_scale_pct = pct.clone();
            }
        });
    }
}

pub const WINDOW_WIDTH: u32 = 1050;
pub const WINDOW_HEIGHT: u32 = 750;

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (WINDOW_WIDTH, WINDOW_HEIGHT))
}

pub(crate) fn create(
    params: Arc<WavetableFilterParams>,
    wavetable_path: Arc<std::sync::Mutex<String>>,
    should_reload: Arc<std::sync::atomic::AtomicBool>,
    shared_wavetable: Arc<std::sync::Mutex<crate::wavetable::Wavetable>>,
    wavetable_version: Arc<std::sync::atomic::AtomicU32>,
    editor_state: Arc<ViziaState>,
    shared_input_spectrum: Arc<std::sync::Mutex<(f32, Vec<f32>)>>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        cx.add_stylesheet(include_str!("style.css"))
            .expect("Failed to load stylesheet");

        let initial_path = wavetable_path.lock().unwrap().clone();
        let initial_scale = cx.user_scale_factor();
        nih_plug::nih_log!("[SCALE] Editor opened, cx.user_scale_factor() = {}", initial_scale);
        let initial_scale_pct = format!("{}%", (initial_scale * 100.0).round() as u32);

        Data {
            params: params.clone(),
            wavetable_path: initial_path,
            wavetable_version: 0,
            status_message: String::from("Ready"),
            ui_scale_pct: initial_scale_pct,
        }
        .build(cx);

        let wt_path = wavetable_path.clone();
        let reload_flag = should_reload.clone();
        let shared_wt = shared_wavetable.clone();
        let wt_version = wavetable_version.clone();

        VStack::new(cx, |cx| {
            // Header row: title + scale controls
            HStack::new(cx, |cx| {
                Label::new(cx, "Wavetable Filter")
                    .font_size(30.0)
                    .width(Stretch(1.0));

                Button::new(
                    cx,
                    |cx| {
                        let current = cx.user_scale_factor();
                        let idx = nearest_scale_idx(current);
                        if idx > 0 {
                            let new_scale = SCALE_STEPS[idx - 1];
                            cx.set_user_scale_factor(new_scale);
                            let p = Data::params.get(cx);
                            set_scale_param(cx, &p.ui_scale, new_scale);
                            cx.emit(DataEvent::SetUiScalePct(
                                format!("{}%", (new_scale * 100.0).round() as u32),
                            ));
                        }
                    },
                    |cx| Label::new(cx, "-"),
                )
                .width(Pixels(24.0))
                .height(Pixels(24.0))
                .class("scale-btn");

                Label::new(cx, Data::ui_scale_pct)
                    .width(Pixels(48.0))
                    .height(Pixels(24.0))
                    .class("scale-label");

                Button::new(
                    cx,
                    |cx| {
                        let current = cx.user_scale_factor();
                        let idx = nearest_scale_idx(current);
                        if idx < SCALE_STEPS.len() - 1 {
                            let new_scale = SCALE_STEPS[idx + 1];
                            cx.set_user_scale_factor(new_scale);
                            let p = Data::params.get(cx);
                            set_scale_param(cx, &p.ui_scale, new_scale);
                            cx.emit(DataEvent::SetUiScalePct(
                                format!("{}%", (new_scale * 100.0).round() as u32),
                            ));
                        }
                    },
                    |cx| Label::new(cx, "+"),
                )
                .width(Pixels(24.0))
                .height(Pixels(24.0))
                .class("scale-btn");
            })
            .height(Pixels(40.0))
            .col_between(Pixels(4.0))
            .child_top(Stretch(1.0))
            .child_bottom(Stretch(1.0));

            // Status message
            Label::new(cx, Data::status_message).height(Pixels(20.0));

            // Wavetable name + browse
            HStack::new(cx, |cx| {
                Label::new(cx, "Wavetable:")
                    .width(Pixels(80.0))
                    .height(Pixels(30.0));

                Label::new(
                    cx,
                    Data::wavetable_path.map(|p| {
                        std::path::Path::new(p)
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("(none)")
                            .to_string()
                    }),
                )
                .width(Stretch(1.0))
                .height(Pixels(30.0))
                .class("path-display")
                .tooltip(|cx| {
                    Label::new(cx, Data::wavetable_path);
                });

                let wt_path_inner = wt_path.clone();
                let reload_flag_inner = reload_flag.clone();
                let shared_wt_inner = shared_wt.clone();
                let wt_version_inner = wt_version.clone();

                Button::new(
                    cx,
                    move |cx| {
                        nih_plug::nih_log!("Browse button clicked");
                        let mut dialog = rfd::FileDialog::new()
                            .add_filter("Wavetable files", &["wav", "wt"]);
                        if let Ok(current) = wt_path_inner.lock() {
                            if let Some(dir) = std::path::Path::new(current.as_str()).parent() {
                                if dir.exists() {
                                    dialog = dialog.set_directory(dir);
                                }
                            }
                        }
                        if let Some(path) = dialog.pick_file()
                        {
                            nih_plug::nih_log!("File selected: {:?}", path);
                            if let Some(path_str) = path.to_str() {
                                let path_string = path_str.to_string();
                                nih_plug::nih_log!(
                                    "Loading wavetable from GUI thread: {}",
                                    path_string
                                );

                                // Load the wavetable immediately on the GUI thread
                                cx.emit(DataEvent::SetStatus(format!(
                                    "Loading {}...",
                                    path_string
                                )));

                                match crate::wavetable::Wavetable::from_file(&path_string) {
                                    Ok(new_wavetable) => {
                                        let msg = format!(
                                            "Loaded: {} frames x {} samples",
                                            new_wavetable.frame_count, new_wavetable.frame_size
                                        );
                                        cx.emit(DataEvent::SetStatus(msg));

                                        // Update the shared wavetable for UI display
                                        if let Ok(mut shared) = shared_wt_inner.lock() {
                                            *shared = new_wavetable;
                                        }

                                        // Increment version to trigger UI redraw
                                        let new_version = wt_version_inner
                                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                            + 1;

                                        // Update path and set reload flag for audio thread
                                        if let Ok(mut wt) = wt_path_inner.lock() {
                                            *wt = path_string.clone();
                                        }
                                        reload_flag_inner
                                            .store(true, std::sync::atomic::Ordering::Relaxed);

                                        // Update UI - emit both events
                                        cx.emit(DataEvent::SetWavetablePath(path_string));
                                        cx.emit(DataEvent::WavetableChanged(new_version));
                                    }
                                    Err(e) => {
                                        cx.emit(DataEvent::SetStatus(format!("Error: {}", e)));
                                    }
                                }
                            }
                        } else {
                            nih_plug::nih_log!("File dialog cancelled");
                        }
                    },
                    |cx| Label::new(cx, "Browse..."),
                )
                .width(Pixels(100.0))
                .height(Pixels(30.0));
            })
            .height(Pixels(40.0))
            .col_between(Pixels(10.0));

            // Mode selection row
            HStack::new(cx, |cx| {
                Label::new(cx, "Mode")
                    .width(Pixels(50.0))
                    .height(Pixels(30.0));
                ParamSlider::new(cx, Data::params, |params| &params.mode)
                    .set_style(ParamSliderStyle::CurrentStepLabeled { even: true })
                    .width(Pixels(200.0))
                    .class("mode-slider");
            })
            .height(Pixels(40.0))
            .col_between(Pixels(10.0));

            // Visualization area with dials below each display
            HStack::new(cx, |cx| {
                // Left column: wavetable view + frame position dial
                VStack::new(cx, |cx| {
                    Label::new(cx, "3D Wavetable View")
                        .class("section-title")
                        .height(Pixels(25.0));
                    WavetableView::new(
                        cx,
                        params.clone(),
                        shared_wavetable.clone(),
                        wavetable_version.clone(),
                    )
                    .height(Pixels(220.0));

                    HStack::new(cx, |cx| {
                        ParamDial::new(cx, Data::params, |params| &params.frame_position)
                            .width(Pixels(110.0))
                            .height(Pixels(110.0));
                    })
                    .height(Pixels(120.0))
                    .child_top(Pixels(6.0));
                })
                .width(Stretch(1.0));

                // Right column: filter response + cutoff/resonance dials + drive/mix
                VStack::new(cx, |cx| {
                    Label::new(cx, "Filter Response")
                        .class("section-title")
                        .height(Pixels(25.0));
                    FilterResponseView::new(
                        cx,
                        params.clone(),
                        shared_wavetable.clone(),
                        shared_input_spectrum.clone(),
                    )
                    .height(Pixels(220.0));

                    HStack::new(cx, |cx| {
                        ParamDial::new(cx, Data::params, |params| &params.frequency)
                            .width(Pixels(110.0))
                            .height(Pixels(110.0));
                        ParamDial::new(cx, Data::params, |params| &params.resonance)
                            .width(Pixels(110.0))
                            .height(Pixels(110.0));
                    })
                    .col_between(Pixels(10.0))
                    .height(Pixels(120.0))
                    .child_top(Pixels(6.0));

                    // Drive and Mix in the lower right
                    HStack::new(cx, |cx| {
                        ParamDial::new(cx, Data::params, |params| &params.drive)
                            .width(Pixels(110.0))
                            .height(Pixels(110.0));
                        ParamDial::new(cx, Data::params, |params| &params.mix)
                            .width(Pixels(110.0))
                            .height(Pixels(110.0));
                    })
                    .col_between(Pixels(10.0))
                    .height(Pixels(120.0))
                    .child_left(Stretch(1.0))
                    .child_top(Stretch(1.0));
                })
                .width(Stretch(1.0));
            })
            .col_between(Pixels(10.0))
            .child_top(Pixels(10.0));
        })
        .row_between(Pixels(10.0))
        .child_left(Pixels(20.0))
        .child_right(Pixels(20.0))
        .child_top(Pixels(20.0))
        .child_bottom(Pixels(20.0));
    })
}
