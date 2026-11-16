use super::*;

impl EffectEditor {
    pub(in crate::editor) fn draw_follower_popup(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut widgets::TextRenderer,
        geometry: EditorGeometry,
    ) {
        if !self.follower.open {
            return;
        }
        let Some(owner) = self.follower.owner else {
            return;
        };
        let Some(lay) = layout(geometry, self.follower.scroll) else {
            return;
        };
        let s = lay.scale;
        let p = lay.panel;
        let font = (12.0 * s).max(10.0);
        widgets::draw_rect(pm, p.0, p.1, p.2, p.3, widgets::color_bg());
        widgets::draw_rect_outline(pm, p.0, p.1, p.2, p.3, widgets::color_accent(), 1.0);
        let title = format!("Follower · {}", ["Amp", "1", "2", "3"][owner.slot]);
        bounded_text(
            tr,
            pm,
            (p.0 + 8.0, p.1 + (22.0 * s).max(17.0)),
            &title,
            (16.0 * s).max(12.0),
            lay.close.0 - p.0 - 12.0,
            widgets::color_text(),
        );
        bounded_text(
            tr,
            pm,
            (p.0 + 8.0, p.1 + (42.0 * s).max(34.0)),
            "Mode/Amount: this curve; response: shared",
            (11.0 * s).max(10.0),
            p.2 - 16.0,
            widgets::color_muted(),
        );
        let r = lay.close;
        widgets::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        let cfg = owner.config;
        for (row, rect) in lay.rows.iter().enumerate() {
            let Some(r) = rect else {
                continue;
            };
            if row < 3 {
                let label = match row {
                    0 => format!(
                        "This curve: {}",
                        if cfg.routes[owner.slot].enabled {
                            "On"
                        } else {
                            "Off"
                        }
                    ),
                    1 => format!(
                        "Curve mode: {}",
                        MODES[mode_index(cfg.routes[owner.slot].mode)]
                    ),
                    _ => format!(
                        "Shared input: {}",
                        if cfg.detector.input == DetectorInput::Main {
                            "Main"
                        } else {
                            "Sidechain"
                        }
                    ),
                };
                widgets::draw_button(
                    pm,
                    tr,
                    r.0,
                    r.1,
                    r.2,
                    r.3,
                    &label,
                    if row == 0 {
                        cfg.routes[owner.slot].enabled
                    } else {
                        self.follower.focus == row
                    },
                    false,
                );
            } else {
                let field = Field::ALL[row - 3];
                let font = (r.3 * 0.5).max(10.0);
                let edit = self.follower.text.active_for(&field).map(|mut text| {
                    let width = (r.2 * 0.4).min(tr.text_width("-9999.99", font));
                    while !text.is_empty() && tr.text_width(text, font) > width {
                        let first = text.chars().next().map_or(0, char::len_utf8);
                        text = &text[first..];
                    }
                    text
                });
                widgets::draw_slider(
                    pm,
                    tr,
                    r.0,
                    r.1,
                    r.2,
                    r.3,
                    field.label(),
                    &field.display(cfg, owner.slot),
                    field.norm(cfg, owner.slot),
                    edit,
                    self.follower.text.caret_visible(),
                );
                if self.follower.focus == row {
                    widgets::draw_rect_outline(
                        pm,
                        r.0,
                        r.1,
                        r.2,
                        r.3,
                        widgets::color_accent(),
                        1.0,
                    );
                }
            }
        }
        if lay.visible < ROWS {
            let r = lay.body;
            let x = r.0 + r.2 - 4.0;
            let h = r.3 * lay.visible as f32 / ROWS as f32;
            let y = r.1 + (r.3 - h) * self.follower.scroll as f32 / (ROWS - lay.visible) as f32;
            widgets::draw_rect(pm, x, r.1, 3.0, r.3, widgets::color_control_bg());
            widgets::draw_rect(pm, x, y, 3.0, h, widgets::color_muted());
        }
        let reading = self.follower_reading();
        let f = lay.footer;
        let line = (25.0 * s).max(24.0);
        let meters = [
            (reading.map(|r| r.level), "Input"),
            (reading.map(|r| r.envelope), "Envelope"),
        ];
        for (i, (value, label)) in meters.into_iter().enumerate() {
            let y = f.1 + i as f32 * line;
            let text = value.map_or_else(
                || format!("{label}: unavailable"),
                |value| {
                    if i == 0 {
                        if value > 0.0 {
                            format!("Input: {:.1} dB", 20.0 * value.log10())
                        } else {
                            "Input: -inf dB".into()
                        }
                    } else {
                        format!("Envelope: {:.3}", value)
                    }
                },
            );
            bounded_text(
                tr,
                pm,
                (f.0, y + font),
                &text,
                font,
                f.2,
                widgets::color_text(),
            );
            let bar = (f.0, y + font + 3.0, f.2, (5.0 * s).max(4.0));
            widgets::draw_rect(pm, bar.0, bar.1, bar.2, bar.3, widgets::color_control_bg());
            if let Some(value) = value {
                let norm = if i == 0 {
                    ((20.0 * value.max(1e-8).log10() + 60.0) / 84.0).clamp(0.0, 1.0)
                } else {
                    value.clamp(0.0, 1.0)
                };
                widgets::draw_rect(
                    pm,
                    bar.0,
                    bar.1,
                    bar.2 * norm,
                    bar.3,
                    widgets::color_accent(),
                );
            }
            if i == 0
                && let Some(db) = cfg.detector.level_db
            {
                let x = bar.0 + bar.2 * ((db + 60.0) / 84.0).clamp(0.0, 1.0);
                widgets::draw_rect(pm, x, bar.1 - 1.0, 1.0, bar.3 + 2.0, widgets::color_text());
            }
        }
        let r = lay.reset;
        widgets::draw_button(pm, tr, r.0, r.1, r.2, r.3, "Reset response", false, false);
        let hint = match cfg.routes[owner.slot].mode {
            FollowerMode::Follow => "Follow replaces curve",
            FollowerMode::Add => "Curve + audio response",
            FollowerMode::Multiply => "Curve × audio response",
        };
        bounded_text(
            tr,
            pm,
            (r.0 + r.2 + 6.0, r.1 + r.3 * 0.7),
            hint,
            (10.0 * s).max(10.0),
            (f.2 - r.2 - 8.0).max(0.0),
            widgets::color_muted(),
        );
        if let Some(choice) = self.follower.choice() {
            widgets::dropdown::draw_dropdown_popup(
                pm,
                tr,
                &self.follower.dropdown,
                choice.items(),
                geometry.window_size,
            );
        }
    }
}
fn mode_index(mode: FollowerMode) -> usize {
    match mode {
        FollowerMode::Follow => 0,
        FollowerMode::Add => 1,
        FollowerMode::Multiply => 2,
    }
}

impl Session<'_> {
    pub(in crate::editor) fn draw_follower_button(&mut self, pm: &mut tiny_skia::Pixmap) {
        let strip = self
            .state
            .mseg_edit
            .layout(self.layout.mseg_pane, self.scale_factor)
            .strip;
        let r = widgets::mseg::follower_button_rect(strip, self.scale_factor);
        if r.2 <= 0.0 {
            return;
        }
        widgets::draw_button(
            pm,
            self.text_renderer,
            r.0,
            r.1,
            r.2,
            r.3,
            "Follow",
            self.modulation.follower.routes[self.state.selected_mseg.min(3)].enabled,
            false,
        );
    }
}
