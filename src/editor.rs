use nih_plug::prelude::Editor;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::widgets::*;
use nih_plug_vizia::{create_vizia_editor, ViziaState, ViziaTheming};
use std::sync::Arc;

use crate::WavetableFilterParams;

#[derive(Lens)]
struct Data {
    params: Arc<WavetableFilterParams>,
}

impl Model for Data {}

const WINDOW_WIDTH: u32 = 700;
const WINDOW_HEIGHT: u32 = 500;

pub(crate) fn default_state() -> Arc<ViziaState> {
    ViziaState::new(|| (WINDOW_WIDTH, WINDOW_HEIGHT))
}

pub(crate) fn create(params: Arc<WavetableFilterParams>) -> Option<Box<dyn Editor>> {
    create_vizia_editor(
        default_state(),
        ViziaTheming::Custom,
        move |cx, _| {
            cx.add_stylesheet(include_str!("style.css"))
                .expect("Failed to load stylesheet");

            Data {
                params: params.clone(),
            }
            .build(cx);

            VStack::new(cx, |cx| {
                // Header with title and load button
                HStack::new(cx, |cx| {
                    Label::new(cx, "Wavetable Filter")
                        .font_size(30.0)
                        .width(Stretch(1.0));

                    Label::new(cx, "Load Wavetable: Drag & drop .wav or .wt file here")
                        .font_size(12.0)
                        .width(Pixels(300.0));
                })
                .height(Pixels(50.0))
                .col_between(Pixels(20.0));

                // Frequency control
                HStack::new(cx, |cx| {
                    Label::new(cx, "Frequency")
                        .width(Pixels(100.0))
                        .height(Pixels(30.0));
                    ParamSlider::new(cx, Data::params, |params| &params.frequency);
                })
                .height(Pixels(40.0))
                .col_between(Pixels(10.0));

                // Frame position control
                HStack::new(cx, |cx| {
                    Label::new(cx, "Frame Position")
                        .width(Pixels(100.0))
                        .height(Pixels(30.0));
                    ParamSlider::new(cx, Data::params, |params| &params.frame_position);
                })
                .height(Pixels(40.0))
                .col_between(Pixels(10.0));

                // Mix control
                HStack::new(cx, |cx| {
                    Label::new(cx, "Mix")
                        .width(Pixels(100.0))
                        .height(Pixels(30.0));
                    ParamSlider::new(cx, Data::params, |params| &params.mix);
                })
                .height(Pixels(40.0))
                .col_between(Pixels(10.0));

                // Drive control
                HStack::new(cx, |cx| {
                    Label::new(cx, "Drive")
                        .width(Pixels(100.0))
                        .height(Pixels(30.0));
                    ParamSlider::new(cx, Data::params, |params| &params.drive);
                })
                .height(Pixels(40.0))
                .col_between(Pixels(10.0));

                // Visualization area - split into two sections
                HStack::new(cx, |cx| {
                    // 3D Wavetable view (left side)
                    VStack::new(cx, |cx| {
                        Label::new(cx, "3D Wavetable View")
                            .class("section-title")
                            .height(Pixels(25.0));
                        Element::new(cx)
                            .class("wavetable-3d-view")
                            .height(Pixels(220.0))
                            .background_color(Color::rgb(20, 22, 28));
                    })
                    .width(Stretch(1.0))
                    .height(Pixels(250.0));

                    // Filter frequency response (right side)
                    VStack::new(cx, |cx| {
                        Label::new(cx, "Filter Response")
                            .class("section-title")
                            .height(Pixels(25.0));
                        Element::new(cx)
                            .class("filter-response-view")
                            .height(Pixels(220.0))
                            .background_color(Color::rgb(20, 22, 28));
                    })
                    .width(Stretch(1.0))
                    .height(Pixels(250.0));
                })
                .col_between(Pixels(10.0))
                .child_top(Pixels(20.0));
            })
            .row_between(Pixels(10.0))
            .child_left(Pixels(20.0))
            .child_right(Pixels(20.0))
            .child_top(Pixels(20.0))
            .child_bottom(Pixels(20.0));
        },
    )
}
