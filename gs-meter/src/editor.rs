use nih_plug::prelude::*;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crate::{GsMeterParams, MeterReadings};

const WINDOW_WIDTH: u32 = 400;
const WINDOW_HEIGHT: u32 = 540;

const SCALE_STEPS: &[f64] = &[1.0, 1.25, 1.5, 1.75, 2.0, 2.25, 2.5, 2.75, 3.0];

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

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (WINDOW_WIDTH, WINDOW_HEIGHT))
}

#[derive(Lens, Clone)]
struct Data {
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<std::sync::atomic::AtomicBool>,
    ui_scale_pct: String,
}

enum DataEvent {
    Reset,
    SetGainFromReading(ReadingKind),
    SetUiScalePct(String),
}

#[derive(Clone, Copy)]
enum ReadingKind {
    PeakMax,
    TruePeakMax,
    RmsIntegrated,
    RmsMomentary,
    RmsMomentaryMax,
}

impl Model for Data {
    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|data_event, _| match data_event {
            DataEvent::Reset => {
                self.should_reset.store(true, Ordering::Relaxed);
            }
            DataEvent::SetGainFromReading(kind) => {
                let meter_db = match kind {
                    ReadingKind::PeakMax => {
                        MeterReadings::load_db(&self.readings.peak_max_db)
                    }
                    ReadingKind::TruePeakMax => {
                        MeterReadings::load_db(&self.readings.true_peak_max_db)
                    }
                    ReadingKind::RmsIntegrated => {
                        MeterReadings::load_db(&self.readings.rms_integrated_db)
                    }
                    ReadingKind::RmsMomentary => {
                        MeterReadings::load_db(&self.readings.rms_momentary_db)
                    }
                    ReadingKind::RmsMomentaryMax => {
                        MeterReadings::load_db(&self.readings.rms_momentary_max_db)
                    }
                };
                if meter_db <= -100.0 {
                    return;
                }
                let reference = self.params.reference_level.value();
                let target_gain_db = reference - meter_db;
                let target_gain_linear = nih_plug::util::db_to_gain(target_gain_db);
                let normalized = self.params.gain.preview_normalized(target_gain_linear);
                cx.emit(RawParamEvent::BeginSetParameter(self.params.gain.as_ptr()));
                cx.emit(RawParamEvent::SetParameterNormalized(
                    self.params.gain.as_ptr(),
                    normalized,
                ));
                cx.emit(RawParamEvent::EndSetParameter(self.params.gain.as_ptr()));
            }
            DataEvent::SetUiScalePct(pct) => {
                self.ui_scale_pct = pct.clone();
            }
        });
    }
}

fn format_db(db: f32) -> String {
    if db <= -100.0 {
        "-inf dB".to_string()
    } else {
        format!("{:.1} dB", db)
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn create(
    params: Arc<GsMeterParams>,
    readings: Arc<MeterReadings>,
    should_reset: Arc<std::sync::atomic::AtomicBool>,
    editor_state: Arc<ViziaState>,
) -> Option<Box<dyn Editor>> {
    create_vizia_editor(editor_state, ViziaTheming::Custom, move |cx, _| {
        nih_plug_widgets::load_style(cx);

        let initial_scale = cx.user_scale_factor();
        let initial_scale_pct = format!("{}%", (initial_scale * 100.0).round() as u32);

        Data {
            params: params.clone(),
            readings: readings.clone(),
            should_reset: should_reset.clone(),
            ui_scale_pct: initial_scale_pct,
        }
        .build(cx);

        VStack::new(cx, |cx| {
            // Header row: title + scale controls
            HStack::new(cx, |cx| {
                Label::new(cx, "GS Meter")
                    .font_size(24.0)
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
            .height(Pixels(35.0))
            .col_between(Pixels(4.0))
            .child_top(Stretch(1.0))
            .child_bottom(Stretch(1.0));

            // Channel mode
            HStack::new(cx, |cx| {
                Label::new(cx, "Channel")
                    .width(Pixels(80.0))
                    .height(Pixels(28.0));
                ParamSlider::new(cx, Data::params, |p| &p.channel_mode)
                    .set_style(ParamSliderStyle::CurrentStepLabeled { even: true })
                    .width(Pixels(200.0));
            })
            .height(Pixels(35.0))
            .col_between(Pixels(10.0));

            // Gain control
            HStack::new(cx, |cx| {
                Label::new(cx, "Gain")
                    .width(Pixels(80.0))
                    .height(Pixels(28.0));
                ParamSlider::new(cx, Data::params, |p| &p.gain)
                    .width(Pixels(200.0));
            })
            .height(Pixels(35.0))
            .col_between(Pixels(10.0));

            // Reference level
            HStack::new(cx, |cx| {
                Label::new(cx, "Reference")
                    .width(Pixels(80.0))
                    .height(Pixels(28.0));
                ParamSlider::new(cx, Data::params, |p| &p.reference_level)
                    .width(Pixels(200.0));
            })
            .height(Pixels(35.0))
            .col_between(Pixels(10.0));

            // RMS Window
            HStack::new(cx, |cx| {
                Label::new(cx, "RMS Window")
                    .width(Pixels(80.0))
                    .height(Pixels(28.0));
                ParamSlider::new(cx, Data::params, |p| &p.rms_window_ms)
                    .width(Pixels(200.0));
            })
            .height(Pixels(35.0))
            .col_between(Pixels(10.0));

            // Meter readings header
            Label::new(cx, "Readings")
                .font_size(16.0)
                .height(Pixels(30.0));

            // Peak Max
            meter_row(
                cx,
                "Peak Max",
                Data::readings.map(|r| format_db(MeterReadings::load_db(&r.peak_max_db))),
                ReadingKind::PeakMax,
            );

            // True Peak Max
            meter_row(
                cx,
                "True Peak",
                Data::readings.map(|r| format_db(MeterReadings::load_db(&r.true_peak_max_db))),
                ReadingKind::TruePeakMax,
            );

            // RMS Integrated
            meter_row(
                cx,
                "RMS (Int)",
                Data::readings.map(|r| format_db(MeterReadings::load_db(&r.rms_integrated_db))),
                ReadingKind::RmsIntegrated,
            );

            // RMS Momentary
            meter_row(
                cx,
                "RMS (Mom)",
                Data::readings.map(|r| format_db(MeterReadings::load_db(&r.rms_momentary_db))),
                ReadingKind::RmsMomentary,
            );

            // RMS Momentary Max
            meter_row(
                cx,
                "RMS Max",
                Data::readings.map(|r| format_db(MeterReadings::load_db(&r.rms_momentary_max_db))),
                ReadingKind::RmsMomentaryMax,
            );

            // Crest Factor (no gain button)
            HStack::new(cx, |cx| {
                Label::new(cx, "Crest")
                    .width(Pixels(80.0))
                    .height(Pixels(28.0));
                Label::new(
                    cx,
                    Data::readings.map(|r| {
                        let db = MeterReadings::load_db(&r.crest_factor_db);
                        if db <= -100.0 {
                            "-- dB".to_string()
                        } else {
                            format!("{:.1} dB", db)
                        }
                    }),
                )
                .width(Pixels(120.0))
                .height(Pixels(28.0));
            })
            .height(Pixels(35.0))
            .col_between(Pixels(10.0));

            // Reset button
            Button::new(
                cx,
                |cx| cx.emit(DataEvent::Reset),
                |cx| Label::new(cx, "Reset"),
            )
            .width(Pixels(100.0))
            .height(Pixels(30.0));
        })
        .row_between(Pixels(4.0))
        .child_left(Pixels(20.0))
        .child_right(Pixels(20.0))
        .child_top(Pixels(15.0))
        .child_bottom(Pixels(15.0));
    })
}

fn meter_row<L>(cx: &mut Context, label: &str, value_lens: L, kind: ReadingKind)
where
    L: Lens<Target = String>,
{
    HStack::new(cx, |cx| {
        Label::new(cx, label)
            .width(Pixels(80.0))
            .height(Pixels(28.0));
        Label::new(cx, value_lens)
            .width(Pixels(120.0))
            .height(Pixels(28.0));
        Button::new(
            cx,
            move |cx| cx.emit(DataEvent::SetGainFromReading(kind)),
            |cx| Label::new(cx, "→ Gain"),
        )
        .width(Pixels(70.0))
        .height(Pixels(24.0));
    })
    .height(Pixels(35.0))
    .col_between(Pixels(10.0));
}
