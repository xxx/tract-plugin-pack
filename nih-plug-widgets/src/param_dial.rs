use nih_plug::prelude::Param;
use nih_plug_vizia::vizia::prelude::*;
use nih_plug_vizia::vizia::vg;
use nih_plug_vizia::widgets::param_base::ParamWidgetBase;
use nih_plug_vizia::widgets::util::ModifiersExt;

/// When shift+dragging, one pixel corresponds to this much normalized change.
const GRANULAR_DRAG_MULTIPLIER: f32 = 0.1;

/// Start angle in radians: 135deg in math convention (225deg clockwise from 3-o'clock).
const START_ANGLE: f32 = std::f32::consts::PI * 0.75; // 135deg
/// End angle in radians: 405deg in math convention (135deg + 270deg).
const END_ANGLE: f32 = std::f32::consts::PI * 0.75 + std::f32::consts::PI * 1.5; // 405deg

#[derive(Lens)]
pub struct ParamDial {
    param_base: ParamWidgetBase,

    drag_active: bool,
    /// Y coordinate where the drag started.
    drag_start_y: f32,
    /// Normalized value when the drag started.
    drag_start_value: f32,
    /// Shift+drag state: if Some, contains the starting Y and value for granular dragging.
    granular_drag_status: Option<GranularDragStatus>,

    scrolled_lines: f32,
}

#[derive(Debug, Clone, Copy)]
pub struct GranularDragStatus {
    starting_y_coordinate: f32,
    starting_value: f32,
}

impl ParamDial {
    pub fn new<L, Params, P, FMap>(
        cx: &mut Context,
        params: L,
        params_to_param: FMap,
    ) -> Handle<'_, Self>
    where
        L: Lens<Target = Params> + Clone,
        Params: 'static,
        P: Param + 'static,
        FMap: Fn(&Params) -> &P + Copy + 'static,
    {
        Self {
            param_base: ParamWidgetBase::new(cx, params, params_to_param),

            drag_active: false,
            drag_start_y: 0.0,
            drag_start_value: 0.0,
            granular_drag_status: None,

            scrolled_lines: 0.0,
        }
        .build(
            cx,
            ParamWidgetBase::build_view(params, params_to_param, move |cx, param_data| {
                // Name label above the arc
                Label::new(cx, param_data.param().name())
                    .class("dial-label")
                    .hoverable(false);

                // Value text below the arc
                let value_lens = param_data.make_lens(|param| {
                    param.normalized_value_to_string(param.unmodulated_normalized_value(), true)
                });
                Label::new(cx, value_lens)
                    .class("dial-value")
                    .hoverable(false);
            }),
        )
    }

    /// Map a normalized value [0, 1] to an angle in radians.
    fn value_to_angle(normalized: f32) -> f32 {
        START_ANGLE + normalized.clamp(0.0, 1.0) * (END_ANGLE - START_ANGLE)
    }
}

impl View for ParamDial {
    fn element(&self) -> Option<&'static str> {
        Some("param-dial")
    }

    fn event(&mut self, cx: &mut EventContext, event: &mut Event) {
        event.map(|window_event, meta| match window_event {
            WindowEvent::MouseDown(MouseButton::Left) => {
                if cx.modifiers().command() {
                    // Ctrl/Cmd+Click: reset to default
                    self.param_base.begin_set_parameter(cx);
                    self.param_base
                        .set_normalized_value(cx, self.param_base.default_normalized_value());
                    self.param_base.end_set_parameter(cx);
                } else {
                    self.drag_active = true;
                    cx.capture();
                    cx.focus();
                    cx.set_active(true);

                    self.param_base.begin_set_parameter(cx);
                    self.drag_start_y = cx.mouse().cursory;
                    self.drag_start_value = self.param_base.unmodulated_normalized_value();

                    if cx.modifiers().shift() {
                        self.granular_drag_status = Some(GranularDragStatus {
                            starting_y_coordinate: cx.mouse().cursory,
                            starting_value: self.drag_start_value,
                        });
                    } else {
                        self.granular_drag_status = None;
                    }
                }
                meta.consume();
            }
            WindowEvent::MouseDoubleClick(MouseButton::Left)
            | WindowEvent::MouseDown(MouseButton::Right)
            | WindowEvent::MouseDoubleClick(MouseButton::Right) => {
                // Double-click and right-click: reset to default
                self.param_base.begin_set_parameter(cx);
                self.param_base
                    .set_normalized_value(cx, self.param_base.default_normalized_value());
                self.param_base.end_set_parameter(cx);
                meta.consume();
            }
            WindowEvent::MouseUp(MouseButton::Left) if self.drag_active => {
                self.drag_active = false;
                cx.release();
                cx.set_active(false);
                self.param_base.end_set_parameter(cx);
                meta.consume();
            }
            WindowEvent::MouseMove(_x, y) if self.drag_active => {
                // Vertical drag: up = increase, down = decrease
                // 200px of travel = full 0->1 range (scaled by DPI)
                let pixels_per_full_range = 200.0 / cx.scale_factor();

                if cx.modifiers().shift() {
                    let status =
                        *self
                            .granular_drag_status
                            .get_or_insert(GranularDragStatus {
                                starting_y_coordinate: *y,
                                starting_value: self
                                    .param_base
                                    .unmodulated_normalized_value(),
                            });
                    let delta_y = status.starting_y_coordinate - *y;
                    let delta_value =
                        (delta_y / pixels_per_full_range) * GRANULAR_DRAG_MULTIPLIER;
                    let new_value = (status.starting_value + delta_value).clamp(0.0, 1.0);
                    self.param_base.set_normalized_value(cx, new_value);
                } else {
                    self.granular_drag_status = None;
                    let delta_y = self.drag_start_y - *y;
                    let delta_value = delta_y / pixels_per_full_range;
                    let new_value =
                        (self.drag_start_value + delta_value).clamp(0.0, 1.0);
                    self.param_base.set_normalized_value(cx, new_value);
                }
            }
            WindowEvent::KeyUp(_, Some(Key::Shift))
                if self.drag_active && self.granular_drag_status.is_some() =>
            {
                // Snap out of granular drag: update start to current position/value
                self.granular_drag_status = None;
                self.drag_start_y = cx.mouse().cursory;
                self.drag_start_value = self.param_base.unmodulated_normalized_value();
            }
            WindowEvent::MouseScroll(_scroll_x, scroll_y) => {
                self.scrolled_lines += scroll_y;
                if self.scrolled_lines.abs() >= 1.0 {
                    let use_finer_steps = cx.modifiers().shift();

                    if !self.drag_active {
                        self.param_base.begin_set_parameter(cx);
                    }

                    let mut current_value = self.param_base.unmodulated_normalized_value();
                    while self.scrolled_lines >= 1.0 {
                        current_value = self
                            .param_base
                            .next_normalized_step(current_value, use_finer_steps);
                        self.param_base.set_normalized_value(cx, current_value);
                        self.scrolled_lines -= 1.0;
                    }
                    while self.scrolled_lines <= -1.0 {
                        current_value = self
                            .param_base
                            .previous_normalized_step(current_value, use_finer_steps);
                        self.param_base.set_normalized_value(cx, current_value);
                        self.scrolled_lines += 1.0;
                    }

                    if !self.drag_active {
                        self.param_base.end_set_parameter(cx);
                    }
                }
                meta.consume();
            }
            _ => {}
        });
    }

    fn draw(&self, cx: &mut DrawContext, canvas: &mut Canvas) {
        let bounds = cx.bounds();

        // The child Labels (name + value) take up space at the top and bottom.
        // The arc occupies the middle area. Estimate label heights for positioning.
        // Use proportional margins so label clearance scales with UI zoom
        let label_margin = bounds.h * 0.18;
        let arc_size = bounds.w.min(bounds.h - label_margin * 2.0);
        let arc_cx = bounds.x + bounds.w / 2.0;
        let arc_cy = bounds.y + label_margin + arc_size / 2.0;
        let radius = arc_size / 2.0 - 6.0;
        let stroke_width = 3.0;

        // --- Draw background arc (full 270deg track) ---
        let mut bg_path = vg::Path::new();
        bg_path.arc(arc_cx, arc_cy, radius, START_ANGLE, END_ANGLE, vg::Solidity::Hole);
        let bg_paint = vg::Paint::color(vg::Color::rgb(64, 64, 64))
            .with_line_width(stroke_width)
            .with_line_cap(vg::LineCap::Round);
        canvas.stroke_path(&bg_path, &bg_paint);

        // --- Draw value arc (from start to current unmodulated value) ---
        let unmod = self.param_base.unmodulated_normalized_value();
        let modulated = self.param_base.modulated_normalized_value();

        if unmod > 0.001 {
            let value_angle = Self::value_to_angle(unmod);
            let mut val_path = vg::Path::new();
            val_path.arc(
                arc_cx,
                arc_cy,
                radius,
                START_ANGLE,
                value_angle,
                vg::Solidity::Hole,
            );
            let val_paint = vg::Paint::color(vg::Color::rgb(79, 195, 247))
                .with_line_width(stroke_width)
                .with_line_cap(vg::LineCap::Round);
            canvas.stroke_path(&val_path, &val_paint);

            // --- Draw indicator dot at the unmodulated value ---
            let dot_x = arc_cx + radius * value_angle.cos();
            let dot_y = arc_cy + radius * value_angle.sin();
            let mut dot_path = vg::Path::new();
            dot_path.circle(dot_x, dot_y, 4.0);
            canvas.fill_path(
                &dot_path,
                &vg::Paint::color(vg::Color::rgb(79, 195, 247)),
            );
        }

        // --- Draw modulation indicator (arc from unmodulated to modulated value) ---
        if (modulated - unmod).abs() > 0.001 {
            let unmod_angle = Self::value_to_angle(unmod);
            let mod_angle = Self::value_to_angle(modulated);
            // Draw arc from unmodulated to modulated; use Solidity to control direction
            let solidity = if modulated >= unmod {
                vg::Solidity::Hole  // clockwise (positive modulation)
            } else {
                vg::Solidity::Solid // counter-clockwise (negative modulation)
            };
            let mut mod_path = vg::Path::new();
            mod_path.arc(arc_cx, arc_cy, radius, unmod_angle, mod_angle, solidity);
            let mod_paint = vg::Paint::color(vg::Color::rgba(255, 160, 50, 150))
                .with_line_width(stroke_width + 1.0)
                .with_line_cap(vg::LineCap::Round);
            canvas.stroke_path(&mod_path, &mod_paint);

            // Modulated value dot (smaller, orange)
            let mod_dot_x = arc_cx + radius * mod_angle.cos();
            let mod_dot_y = arc_cy + radius * mod_angle.sin();
            let mut mod_dot = vg::Path::new();
            mod_dot.circle(mod_dot_x, mod_dot_y, 3.0);
            canvas.fill_path(
                &mod_dot,
                &vg::Paint::color(vg::Color::rgba(255, 160, 50, 200)),
            );
        }
    }
}
