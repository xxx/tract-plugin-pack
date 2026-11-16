use super::{DragTarget, MsegEditState};
use crate::mseg::render::{in_rect, mseg_hit_test_with_selection, power_buttons, strip_buttons_ex};
use crate::mseg::{MsegData, MsegTool};
use crate::{
    TextRenderer, color_accent, color_bg, color_border, color_control_bg, color_muted, color_text,
    draw_rect, draw_rect_outline,
};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers};
use std::cell::Cell;

type Rect = (f32, f32, f32, f32);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum HelpPage {
    #[default]
    Points,
    Select,
    Shapes,
    View,
}
impl HelpPage {
    pub const ALL: [Self; 4] = [Self::Points, Self::Select, Self::Shapes, Self::View];
    fn label(self) -> &'static str {
        match self {
            Self::Points => "Points",
            Self::Select => "Select",
            Self::Shapes => "Shapes",
            Self::View => "View",
        }
    }
}

#[derive(Default)]
pub(super) struct HelpState {
    open: bool,
    page: HelpPage,
    scroll: Cell<HelpScroll>,
    pointer: Option<(f32, f32)>,
    external: Option<&'static str>,
    bounds: Option<(f32, f32)>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum HelpScroll {
    Top(f32),
    Bottom(f32),
}
impl Default for HelpScroll {
    fn default() -> Self {
        Self::Top(0.0)
    }
}
impl HelpScroll {
    fn advance(self, pixels: f32) -> Self {
        match self {
            Self::Top(offset) => Self::Top((offset + pixels).max(0.0)),
            Self::Bottom(gap) => Self::Bottom((gap - pixels).max(0.0)),
        }
    }
    fn offset(self, overflow: f32) -> f32 {
        match self {
            Self::Top(offset) => offset.min(overflow),
            Self::Bottom(gap) => (overflow - gap).max(0.0),
        }
    }
}

pub fn is_help_shortcut(event: &KeyboardEvent) -> bool {
    event.state == KeyState::Down
        && !event
            .modifiers
            .intersects(Modifiers::CONTROL | Modifiers::META | Modifiers::ALT)
        && (event.key == Key::F1
            || matches!(&event.key, Key::Character(text) if text == "?" || (text == "/" && event.modifiers.contains(Modifiers::SHIFT))))
}

struct Entry {
    gesture: &'static str,
    description: &'static str,
    animated: bool,
}
const fn entry(gesture: &'static str, description: &'static str) -> Entry {
    Entry {
        gesture,
        description,
        animated: false,
    }
}
const fn animated(gesture: &'static str, description: &'static str) -> Entry {
    Entry {
        gesture,
        description,
        animated: true,
    }
}

fn entries(page: HelpPage, curve_only: bool) -> impl Iterator<Item = &'static Entry> {
    let rows: &'static [Entry] = match page {
        HelpPage::Points => {
            const {
                &[
                    entry(
                        "Double-click empty space",
                        "Add a point. Endpoint times stay pinned.",
                    ),
                    entry("Drag a point", "Move it; Shift-drag bypasses snapping."),
                    entry("Double-click a point", "Delete an interior point."),
                    entry(
                        "Drag a tension handle",
                        "Bend the segment between two points.",
                    ),
                    entry(
                        "Right-click",
                        "Toggle a segment's step; a selected point opens Edit instead.",
                    ),
                    entry(
                        "Point / Ctrl/Cmd+Enter",
                        "Select one point to enter exact time, value and tension. Tab applies and advances; Escape discards pending text.",
                    ),
                    entry(
                        "Shape and Weight",
                        "Point offers Curve, S-curve or Step and Hard, Medium or Soft. Soft eases to a flat tangent.",
                    ),
                    entry(
                        "Alt-drag empty space",
                        "Paint stepped points along the time grid.",
                    ),
                ]
            }
        }
        HelpPage::Select => {
            const {
                &[
                    entry(
                        "Drag empty space",
                        "Draw a selection box; Ctrl adds to the selection.",
                    ),
                    entry(
                        "Ctrl-click points",
                        "Add or remove points from the selection.",
                    ),
                    entry("Ctrl/Cmd+A", "Select all points."),
                    entry(
                        "Drag a selected point",
                        "Move the group. Its anchor snaps; Shift gives fine movement.",
                    ),
                    entry(
                        "Selection handles",
                        "Stretch time or height. Alt scales around the center; Ctrl on an edge skews the other axis.",
                    ),
                    entry(
                        "Arrow Left / Right",
                        "Nudge selected points by one time-grid cell; Shift uses a fine step.",
                    ),
                    entry("Delete / Backspace", "Delete selected interior points."),
                    entry(
                        "Escape",
                        "Cancel an active gesture, clear selection and return to Pointer.",
                    ),
                ]
            }
        }
        HelpPage::Shapes => {
            const {
                &[
                    entry("Tool menu", "Choose Pointer, Line, Arc or S-curve pen."),
                    entry(
                        "Pen click / drag",
                        "Click to stamp the remembered shape; drag to set its width and height.",
                    ),
                    entry(
                        "Pen click again",
                        "Repeat the shape after the previous one without moving the pointer.",
                    ),
                    entry(
                        "Shapes menu",
                        "Browse shapes... previews factory curves, shared Favorites and saved Project waves. Enter loads; Shift+Enter fits. Favorites/Project: S stores, Delete clears, R refreshes. Import/export transfers curve files.",
                    ),
                    entry(
                        "Ctrl/Cmd+C / V",
                        "Copy or paste the whole curve across instances and plugins. Shift+paste fits its height.",
                    ),
                    entry(
                        "Ctrl/Cmd+Alt+C / V",
                        "Copy an adjacent phrase or replace the selected interval. Add Shift to paste-merge.",
                    ),
                    entry(
                        "Edit menu",
                        "Flip, nudge, quantize, repeat or mutate points; match or smooth loop seams.",
                    ),
                    entry(
                        "Curve files",
                        "Transfer geometry while retaining the destination's timing, grid and routing settings.",
                    ),
                ]
            }
        }
        HelpPage::View => {
            const {
                &[
                    entry(
                        "Wheel / Shift+wheel",
                        "Zoom time around the pointer; Shift zooms values.",
                    ),
                    entry("Middle-drag", "Pan the view; Escape cancels the pan."),
                    entry(
                        "View menu",
                        "Fit all points or the current selection. Navigation leaves the curve intact.",
                    ),
                    entry(
                        "Snap and Grid",
                        "Choose numeric or triplet divisions; Shift-drag gives fine placement. Edit > Quantize moves existing points to the grid.",
                    ),
                    animated(
                        "Grid > Semitones",
                        "Pitch and logarithmic frequency targets offer musical snapping. Base, Depth and polarity determine reachable notes.",
                    ),
                    animated(
                        "Target / Depth",
                        "Route an assignable curve and scale its modulation. Amp controls gain; Time/Beat and Length set its clock.",
                    ),
                    animated(
                        "Smooth / Trace / Audio",
                        "Multosis/Minitosis offer live lag, measured traces and waveform capture. Audio's Freeze keeps a capture for editing.",
                    ),
                    animated(
                        "Trigger / Detect",
                        "Choose what restarts the curves. Transient (sample) offers detector threshold, filters and Main/Sidechain input.",
                    ),
                    animated(
                        "MIDI waves",
                        "Waves beside playback opens MIDI and host automation setup. Eight notes select Project slots; each curve's Wave lane overrides MIDI. Choose Drawn/MIDI and MIDI Off to edit geometry. Playback ignores saved hold markers while switched.",
                    ),
                    animated(
                        "Ctrl/Cmd+Z",
                        "Undo in Multosis/Minitosis; Shift+Z or Ctrl/Cmd+Y redoes an edit.",
                    ),
                ]
            }
        }
    };
    rows.iter().filter(move |row| !curve_only || !row.animated)
}

#[derive(Clone, Copy)]
pub(crate) struct HelpLayout {
    pub panel: Rect,
    pub close: Rect,
    pub tabs: [Rect; 4],
    pub body: Rect,
    footer: Rect,
}

impl MsegEditState {
    pub fn help_open(&self) -> bool {
        self.help.open
    }

    /// Open view-only help without interrupting another gesture or overlay.
    pub fn open_help(&mut self, scale: f32) -> bool {
        if !scale.is_finite() || scale <= 0.0 || self.is_dragging() || self.has_open_menu() {
            return false;
        }
        self.set_overlay_scale(scale);
        self.clear_hover_hint();
        self.help.open = true;
        self.help.page = HelpPage::Points;
        self.help.scroll.set(HelpScroll::default());
        true
    }

    pub(super) fn close_help(&mut self) {
        self.help.open = false;
    }

    /// Supply native bounds for shared help and curve-browser overlays.
    pub fn set_overlay_bounds(&mut self, bounds: (f32, f32)) {
        self.help.bounds =
            (bounds.0.is_finite() && bounds.1.is_finite() && bounds.0 > 0.0 && bounds.1 > 0.0)
                .then_some(bounds);
    }

    /// Supply native bounds so a guide can span its owning pane.
    pub fn set_help_bounds(&mut self, bounds: (f32, f32)) {
        self.set_overlay_bounds(bounds);
    }

    pub fn clear_hover_hint(&mut self) {
        self.help.pointer = None;
        self.help.external = None;
        self.hover = None;
    }

    pub fn set_external_hint(&mut self, hint: Option<&'static str>) {
        self.help.external = hint;
    }

    pub(super) fn track_hint_pointer(&mut self, x: f32, y: f32, rect: Rect) {
        self.help.pointer = in_rect(rect, x, y).then_some((x, y));
    }

    pub fn context_hint(&self, data: &MsegData, rect: Rect, scale: f32) -> Option<&'static str> {
        use crate::mseg::render::MsegHit;
        if self.curve_browser_open() {
            return Some("Browser: select a thumbnail; Enter loads; Shift+Enter fits; Esc closes");
        }
        if self.help_open() {
            return Some("Guide: wheel scrolls; left/right changes page; Esc closes");
        }
        if self.inspector_node().is_some() {
            return Some("Point: Enter applies; Tab advances; Esc discards text");
        }
        if self.dropdown.is_open() {
            return Some("Menu: arrows choose; Enter applies; Esc closes");
        }
        if let Some(drag) = self.drag {
            return Some(match drag {
                DragTarget::Node(_) | DragTarget::Group { .. } => {
                    "Move points; Shift: unsnapped; Esc: cancel"
                }
                DragTarget::Tension(_) => "Drag to bend; Esc: cancel",
                DragTarget::Transform(_) => "Stretch; Alt: centered; Ctrl: skew; Shift: fine",
                DragTarget::Pen => "Drag size; release to place; Esc: cancel",
                DragTarget::StepDraw => "Paint grid steps; Shift: fine levels",
                DragTarget::Marquee => "Release to select; Ctrl adds to selection",
                DragTarget::Pan => "Middle-drag pans; Esc restores the view",
                DragTarget::Marker(_) => "Hold markers define sustain or loop bounds",
            });
        }
        if let Some(hint) = self.help.external {
            return Some(hint);
        }
        let (x, y) = self.help.pointer?;
        if !in_rect(rect, x, y) {
            return None;
        }
        let layout = self.layout(rect, scale);
        let buttons = strip_buttons_ex(layout.strip, scale, !self.curve_only, self.show_polarity);
        let power = power_buttons(layout.strip, scale);
        for (button, hint) in [
            (power.help, "Open the gesture guide; F1 / ?"),
            (power.tool, "Choose Pointer or a shape pen"),
            (power.shapes, "Factory curves, favorites and curve files"),
            (
                power.edit,
                "Transform, quantize, reuse and close loop seams",
            ),
            (power.view, "Fit the curve, fit selection or open the guide"),
            (power.point, "Select one point to edit exact values"),
            (buttons.snap, "Toggle snapping; Shift-drag bypasses it"),
            (
                buttons.grid,
                if self.curve_only {
                    "Choose numeric or triplet grid divisions"
                } else {
                    "Choose time/value divisions or routed semitones"
                },
            ),
            (buttons.style, "Choose the next Randomize style"),
            (
                buttons.randomize,
                "Generate a curve; the time grid sets density",
            ),
            (
                buttons.polarity,
                "Choose unipolar or bipolar value coordinates",
            ),
            (
                buttons.play_mode,
                "Cyclic repeats; One-shot plays once per trigger",
            ),
        ] {
            if in_rect(button, x, y) {
                return Some(hint);
            }
        }
        if self.geometry_read_only() && in_rect(layout.canvas, x, y) {
            return Some("Wave preview; choose Drawn/MIDI and MIDI Off to edit");
        }
        if self
            .selection_handle_at(data, &layout, scale, x, y)
            .is_some()
        {
            return Some("Drag to stretch; Alt: center; Ctrl: skew; Shift: fine");
        }
        if self.tool() != MsegTool::Pointer && in_rect(layout.canvas, x, y) {
            return Some("Click stamps; drag sizes; click again repeats");
        }
        match mseg_hit_test_with_selection(&layout, data, self.curve_only, scale, x, y, self) {
            MsegHit::Node(index) if index == 0 || index + 1 == data.node_count => {
                Some("Drag endpoint level; its time stays pinned")
            }
            MsegHit::Node(_) => Some("Drag point; double-click deletes; right-click steps"),
            MsegHit::SelectedNode(_) => Some("Drag selection; right-click opens Edit"),
            MsegHit::Tension(_) => Some("Drag to bend this segment; right-click steps"),
            MsegHit::Canvas if self.stepped_draw_held => {
                Some("Alt-drag paints stepped points along the grid")
            }
            MsegHit::Canvas => Some("Drag selects; double-click adds; Alt paints steps"),
            _ => None,
        }
    }

    pub(crate) fn help_layout(&self, rect: Rect, scale: f32) -> Option<HelpLayout> {
        let bounds = self
            .help
            .bounds
            .unwrap_or((rect.0 + rect.2, rect.1 + rect.3));
        let s = scale.max(0.01);
        let pad = (4.0 * s).max(2.0);
        let w = (640.0 * s).min(bounds.0 - 2.0 * pad);
        let h = (460.0 * s).min(bounds.1 - 2.0 * pad);
        if w < 32.0 || h < 40.0 {
            return None;
        }
        let x = (rect.0 + (rect.2 - w) * 0.5).clamp(pad, (bounds.0 - w - pad).max(pad));
        let y = (rect.1 + (rect.3 - h) * 0.5).clamp(pad, (bounds.1 - h - pad).max(pad));
        let inset = (12.0 * s).max(6.0).min(w * 0.1);
        let header = (34.0 * s).max(28.0).min(h * 0.3);
        let tab_h = (26.0 * s).max(20.0).min(h * 0.2);
        let footer_h = (38.0 * s).max(32.0).min(h * 0.3);
        let gap = (4.0 * s).max(2.0);
        let tab_w = ((w - 2.0 * inset - 3.0 * gap) / 4.0).max(0.0);
        let tabs = std::array::from_fn(|i| {
            (
                x + inset + i as f32 * (tab_w + gap),
                y + header,
                tab_w,
                tab_h,
            )
        });
        let body_y = y + header + tab_h + gap;
        let body_bottom = y + h - footer_h;
        Some(HelpLayout {
            panel: (x, y, w, h),
            close: (
                x + w - inset - (24.0 * s).max(14.0),
                y + (4.0 * s).max(2.0),
                (24.0 * s).max(14.0),
                (20.0 * s).max(14.0).min(header - 2.0),
            ),
            tabs,
            body: (
                x + inset,
                body_y,
                (w - 2.0 * inset).max(0.0),
                (body_bottom - body_y).max(0.0),
            ),
            footer: (
                x + inset,
                y + h - footer_h,
                (w - 2.0 * inset).max(0.0),
                footer_h,
            ),
        })
    }

    pub(super) fn help_mouse_down(&mut self, x: f32, y: f32, rect: Rect, scale: f32) {
        let Some(layout) = self.help_layout(rect, scale) else {
            self.close_help();
            return;
        };
        if in_rect(layout.close, x, y) || !in_rect(layout.panel, x, y) {
            self.close_help();
            return;
        }
        for (page, tab) in HelpPage::ALL.into_iter().zip(layout.tabs) {
            if in_rect(tab, x, y) {
                self.help.page = page;
                self.help.scroll.set(HelpScroll::default());
            }
        }
    }

    pub(super) fn help_wheel(&mut self, delta: f32) {
        if delta.is_finite() {
            let line = ((12.0 * self.overlay_scale()).max(10.0) * 1.45).ceil();
            self.help.scroll.set(
                self.help
                    .scroll
                    .get()
                    .advance(-delta.clamp(-10.0, 10.0) * line * 3.0),
            );
        }
    }

    pub(super) fn help_key(&mut self, event: &KeyboardEvent, rect: Rect, scale: f32) {
        if event.state != KeyState::Down {
            return;
        }
        if event.key == Key::Escape || is_help_shortcut(event) {
            self.close_help();
            return;
        }
        let direction = match event.key {
            Key::ArrowLeft => -1,
            Key::ArrowRight => 1,
            Key::Tab => {
                if event.modifiers.contains(Modifiers::SHIFT) {
                    -1
                } else {
                    1
                }
            }
            _ => 0,
        };
        if direction != 0 {
            let index = (self.help.page as i32 + direction).rem_euclid(4) as usize;
            self.help.page = HelpPage::ALL[index];
            self.help.scroll.set(HelpScroll::default());
        } else {
            let line = ((12.0 * scale).max(10.0) * 1.45).ceil();
            let height = self.help_layout(rect, scale).map_or(0.0, |layout| {
                ((layout.body.1 + layout.body.3).floor() - layout.body.1.ceil()).max(0.0)
            });
            let page = (height - 2.0 * line).max(line).min(height);
            let position = self.help.scroll.get();
            self.help.scroll.set(match event.key {
                Key::Home => HelpScroll::default(),
                Key::End => HelpScroll::Bottom(0.0),
                Key::ArrowUp => position.advance(-line),
                Key::ArrowDown => position.advance(line),
                Key::PageUp => position.advance(-page),
                Key::PageDown => position.advance(page),
                _ => position,
            });
        }
    }

    pub(crate) fn draw_help(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut TextRenderer,
        rect: Rect,
        scale: f32,
    ) {
        if !self.help_open() {
            return;
        }
        let Some(layout) = self.help_layout(rect, scale) else {
            return;
        };
        let (x, y, w, h) = layout.panel;
        let s = scale.max(0.01);
        draw_rect(pm, x, y, w, h, color_bg());
        draw_rect_outline(pm, x, y, w, h, color_accent(), 1.0);
        let title = format!("MSEG guide - {}", self.help.page.label());
        let title_size = (16.0 * s).max(11.0);
        let title_width = (layout.close.0 - x - 14.0 * s).max(0.0);
        let title = if tr.text_width(&title, title_size) <= title_width {
            title.as_str()
        } else {
            self.help.page.label()
        };
        draw_fitted(
            tr,
            pm,
            (x + 8.0 * s, y + 22.0 * s.max(0.7)),
            title,
            title_size,
            title_width,
            color_text(),
        );
        let r = layout.close;
        crate::controls::draw_button(pm, tr, r.0, r.1, r.2, r.3, "X", false, false);
        for (index, (page, r)) in HelpPage::ALL.into_iter().zip(layout.tabs).enumerate() {
            let size = (r.3 * 0.5).max(10.0);
            let number = (index + 1).to_string();
            let label = if tr.text_width(page.label(), size) <= r.2 - 8.0 {
                page.label()
            } else {
                number.as_str()
            };
            crate::controls::draw_button(
                pm,
                tr,
                r.0,
                r.1,
                r.2,
                r.3,
                label,
                self.help.page == page,
                false,
            );
        }
        self.draw_help_body(pm, tr, layout.body, s);
        let size = (11.0 * s).max(10.0);
        for (i, text) in [
            "Wheel / Up,Down: scroll; Left,Right: page",
            "Esc / F1: close; Tab: next page",
        ]
        .into_iter()
        .enumerate()
        {
            let baseline = layout.footer.1 + size + 3.0 + i as f32 * size * 1.4;
            if baseline + size * 0.25 < layout.panel.1 + layout.panel.3 {
                draw_fitted(
                    tr,
                    pm,
                    (layout.footer.0, baseline),
                    text,
                    size,
                    layout.footer.2,
                    color_muted(),
                );
            }
        }
    }

    fn draw_help_body(
        &self,
        pm: &mut tiny_skia::Pixmap,
        tr: &mut TextRenderer,
        body: Rect,
        s: f32,
    ) {
        let left = body.0.ceil() as i32;
        let top = body.1.ceil() as i32;
        let width = ((body.0 + body.2).floor() as i32 - left).max(0) as u32;
        let height = ((body.1 + body.3).floor() as i32 - top).max(0) as u32;
        let Some(mut canvas) = tiny_skia::Pixmap::new(width, height) else {
            return;
        };
        canvas.fill(color_control_bg());
        let font = (12.0 * s).max(10.0);
        let line = (font * 1.45).ceil();
        let gap = (8.0 * s).max(6.0);
        let usable = (width as f32 - 14.0 * s.max(1.0)).max(1.0);
        let columns = width as f32 >= 420.0;
        let key_width = if columns { usable * 0.32 } else { usable };
        let desc_width = if columns {
            usable - key_width - gap
        } else {
            usable
        };
        let mut rows = Vec::new();
        let mut total = gap;
        for entry in entries(self.help.page, self.curve_only) {
            let key = wrap(tr, entry.gesture, font, key_width);
            let description = wrap(tr, entry.description, font, desc_width);
            let count = if columns {
                key.len().max(description.len())
            } else {
                key.len() + description.len()
            };
            let row_h = count as f32 * line + gap;
            rows.push((total, row_h, key, description));
            total += row_h;
        }
        let overflow = (total - height as f32).max(0.0);
        let offset = self.help.scroll.get().offset(overflow);
        self.help.scroll.set(HelpScroll::Top(offset));
        for (start, row_h, key, description) in rows {
            for (i, text) in key.iter().enumerate() {
                tr.draw_text(
                    &mut canvas,
                    2.0,
                    start - offset + font + i as f32 * line,
                    text,
                    font,
                    color_accent(),
                );
            }
            let desc_x = if columns { 2.0 + key_width + gap } else { 2.0 };
            let desc_y = start - offset
                + font
                + if columns {
                    0.0
                } else {
                    key.len() as f32 * line
                };
            for (i, text) in description.iter().enumerate() {
                tr.draw_text(
                    &mut canvas,
                    desc_x,
                    desc_y + i as f32 * line,
                    text,
                    font,
                    color_text(),
                );
            }
            let separator = start + row_h - gap * 0.4 - offset;
            if separator >= 0.0 && separator < height as f32 {
                draw_rect(&mut canvas, 2.0, separator, usable, 1.0, color_border());
            }
        }
        if overflow > 0.0 {
            let track_x = (width as f32 - 5.0 * s.max(1.0)).max(0.0);
            let thumb = ((height as f32).powi(2) / total)
                .max(10.0 * s)
                .min(height as f32);
            draw_rect(
                &mut canvas,
                track_x,
                0.0,
                3.0 * s.max(1.0),
                height as f32,
                color_bg(),
            );
            draw_rect(
                &mut canvas,
                track_x,
                offset / overflow * (height as f32 - thumb),
                3.0 * s.max(1.0),
                thumb,
                color_accent(),
            );
        }
        pm.draw_pixmap(
            left,
            top,
            canvas.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            tiny_skia::Transform::identity(),
            None,
        );
    }
}

pub(super) fn draw_fitted(
    tr: &mut TextRenderer,
    pm: &mut tiny_skia::Pixmap,
    at: (f32, f32),
    text: &str,
    size: f32,
    width: f32,
    color: tiny_skia::Color,
) {
    let mut visible = String::new();
    for ch in text.chars() {
        visible.push(ch);
        if tr.text_width(&visible, size) > width {
            visible.pop();
            break;
        }
    }
    tr.draw_text(pm, at.0, at.1, &visible, size, color);
}

pub(super) fn wrap(tr: &mut TextRenderer, text: &str, size: f32, width: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let proposed = if current.is_empty() {
            word.to_owned()
        } else {
            format!("{current} {word}")
        };
        if tr.text_width(&proposed, size) <= width {
            current = proposed;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        for ch in word.chars() {
            current.push(ch);
            if tr.text_width(&current, size) > width && current.len() > ch.len_utf8() {
                current.pop();
                lines.push(std::mem::take(&mut current));
                current.push(ch);
            }
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::editor::{MsegEdit, StripId};
    use crate::mseg::render::{help_button_rect, phase_to_x, value_to_y};

    const RECT: Rect = (20.0, 70.0, 1000.0, 360.0);
    fn key(key: Key) -> KeyboardEvent {
        KeyboardEvent {
            key,
            state: KeyState::Down,
            ..KeyboardEvent::default()
        }
    }
    fn text() -> TextRenderer {
        TextRenderer::new(crate::DEFAULT_FONT)
    }

    #[test]
    fn page_down_keeps_overlapping_visible_content_until_the_last_page() {
        let scale = 0.5;
        let bounds = (520.0, 360.0);
        let rect = (10.0, 35.0, 500.0, 180.0);
        let mut state = MsegEditState::new();
        let mut data = MsegData::default();
        state.set_help_bounds(bounds);
        assert!(state.open_help(scale));
        for _ in 0..3 {
            state.on_key(&key(Key::ArrowRight), &mut data, rect, scale);
        }
        let body = state.help_layout(rect, scale).unwrap().body;
        let (left, top) = (body.0.ceil() as usize, body.1.ceil() as usize);
        let width = (body.0 + body.2).floor() as usize - left - 14;
        let height = (body.1 + body.3).floor() as usize - top;
        let mut tr = text();
        let mut snapshot = |state: &MsegEditState| {
            let mut pm = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
            state.draw_help(&mut pm, &mut tr, rect, scale);
            let mut pixels = Vec::new();
            for y in top..top + height {
                pixels.extend_from_slice(
                    &pm.pixels()
                        [y * pm.width() as usize + left..y * pm.width() as usize + left + width],
                );
            }
            pixels
        };
        let first = snapshot(&state);
        let mut previous = first.clone();
        let mut advances = 0;
        for _ in 0..32 {
            assert_eq!(
                state.on_key(&key(Key::PageDown), &mut data, rect, scale),
                (true, None)
            );
            let next = snapshot(&state);
            if next == previous {
                break;
            }
            let overlap = (1..height - 14).find(|shift| {
                let old = &previous[shift * width..];
                let new = &next[..(height - shift) * width];
                old == new
                    && old
                        .iter()
                        .filter(|pixel| {
                            pixel.red() != 0x2a || pixel.green() != 0x2c || pixel.blue() != 0x32
                        })
                        .count()
                        > 30
            });
            assert!(
                overlap.is_some(),
                "Page Down skipped a visible screen after {advances} advances"
            );
            previous = next;
            advances += 1;
        }
        assert!(advances > 1 && advances < 32);
        state.on_key(&key(Key::Home), &mut data, rect, scale);
        assert_eq!(snapshot(&state), first);
        assert_eq!(data, MsegData::default());
    }

    #[test]
    fn guide_entries_buttons_shortcuts_and_pages_are_view_only() {
        for curve_only in [false, true] {
            let mut state = MsegEditState::with_curve_only(curve_only);
            state.set_help_bounds((1040.0, 720.0));
            let mut data = MsegData::default();
            data.insert_node(0.5, 0.8).unwrap();
            state.select_only(1);
            state.fit_selection(&data);
            let before = data;
            let view = state.viewport();
            let button = help_button_rect(state.layout(RECT, 1.0).strip, 1.0);
            assert_eq!(
                state.on_mouse_down(button.0 + 2.0, button.1 + 2.0, &mut data, RECT, 1.0, false),
                None
            );
            assert!(state.help_open() && state.has_open_menu());
            assert_eq!(
                state.on_key(&key(Key::ArrowRight), &mut data, RECT, 1.0),
                (true, None)
            );
            assert_eq!(state.help.page, HelpPage::Select);
            state.on_key(&key(Key::End), &mut data, RECT, 1.0);
            assert!(state.on_wheel(1000.0, RECT));
            state.on_key(&key(Key::Home), &mut data, RECT, 1.0);
            state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
            assert!(!state.help_open());
            assert_eq!(data, before);
            assert_eq!(state.viewport(), view);
            assert!(state.is_node_selected(1));
            assert_eq!(state.apply_menu(StripId::View, 2, &mut data, RECT), None);
            assert!(state.help_open());
            state.on_right_click(-1.0, -1.0, &mut data, RECT, 1.0);
            assert!(!state.help_open());
            state.on_key(&key(Key::Character("?".into())), &mut data, RECT, 1.0);
            assert!(state.help_open());
            let layout = state.help_layout(RECT, 1.0).unwrap();
            state.on_mouse_down(
                layout.body.0 + 2.0,
                layout.body.1 + 2.0,
                &mut data,
                RECT,
                1.0,
                false,
            );
            assert!(state.help_open());
            state.on_mouse_down(-1.0, -1.0, &mut data, RECT, 1.0, false);
            assert!(!state.help_open());
            assert_eq!(data, before);
        }
    }

    #[test]
    fn help_yields_to_numeric_inspectors_menus_and_active_curve_gestures() {
        let mut state = MsegEditState::new();
        let mut data = MsegData::default();
        state.select_only(0);
        assert!(state.open_point_inspector(&data));
        assert_eq!(
            state.on_key(&key(Key::F1), &mut data, RECT, 1.0),
            (true, None)
        );
        assert!(!state.help_open() && state.inspector_node().is_some());
        state.dismiss_inspector();
        state.open_power_menu(StripId::View, (40.0, 100.0, 80.0, 26.0), (1040.0, 720.0));
        state.on_key(&key(Key::F1), &mut data, RECT, 1.0);
        assert!(!state.help_open() && state.dropdown.is_open());
        state.dropdown.close();
        let layout = state.layout(RECT, 1.0);
        state.on_mouse_down(
            phase_to_x(&layout, 0.0),
            value_to_y(&layout, 0.0),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(state.is_dragging());
        state.on_key(&key(Key::F1), &mut data, RECT, 1.0);
        assert!(!state.help_open() && state.is_dragging());
        state.on_mouse_up(&mut data, RECT, 1.0);
        assert!(state.on_pan_start(
            phase_to_x(&layout, 0.5),
            value_to_y(&layout, 0.5),
            RECT,
            1.0
        ));
        state.on_key(&key(Key::F1), &mut data, RECT, 1.0);
        assert!(!state.help_open() && state.is_panning());
        state.cancel_interaction();
        assert!(state.open_help(1.0));
        state.cancel_interaction();
        assert!(!state.help_open());
    }

    #[test]
    fn hover_hints_use_control_and_geometry_context_without_phantom_or_stale_pointer() {
        let mut state = MsegEditState::new_curve_only();
        let mut data = MsegData::default();
        assert_eq!(state.context_hint(&data, RECT, 1.0), None);
        let layout = state.layout(RECT, 1.0);
        state.on_mouse_move(
            phase_to_x(&layout, 0.0),
            value_to_y(&layout, 0.0),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(
            state
                .context_hint(&data, RECT, 1.0)
                .unwrap()
                .contains("pinned")
        );
        let grid = crate::mseg::render::grid_button_rect(layout.strip, 1.0);
        state.on_mouse_move(grid.0 + 2.0, grid.1 + 2.0, &mut data, RECT, 1.0, false);
        assert!(
            !state
                .context_hint(&data, RECT, 1.0)
                .unwrap()
                .contains("semitones")
        );
        state.set_tool(MsegTool::Line);
        state.on_mouse_move(
            phase_to_x(&layout, 0.0),
            value_to_y(&layout, 0.0),
            &mut data,
            RECT,
            1.0,
            false,
        );
        assert!(
            state
                .context_hint(&data, RECT, 1.0)
                .unwrap()
                .contains("stamps")
        );
        state.clear_hover_hint();
        assert_eq!(state.context_hint(&data, RECT, 1.0), None);
        assert_eq!(data, MsegData::default());
        assert!(entries(HelpPage::View, true).all(|row| !row.animated));
        assert!(entries(HelpPage::View, false).any(|row| row.gesture == "Ctrl/Cmd+Z"));
    }

    #[test]
    fn keyboard_help_round_trip_preserves_pen_preview_repeat_and_selection() {
        let mut state = MsegEditState::new_curve_only();
        let mut data = MsegData::default();
        state.set_tool(MsegTool::SCurve);
        let layout = state.layout(RECT, 1.0);
        let (x, y) = (phase_to_x(&layout, 0.125), value_to_y(&layout, 0.25));
        state.on_mouse_down(x, y, &mut data, RECT, 1.0, false);
        state.on_mouse_up(&mut data, RECT, 1.0);
        let before = data;
        let preview = state.pen_preview().to_vec();
        let selected = state.selection;
        state.on_key(&key(Key::F1), &mut data, RECT, 1.0);
        state.on_key(&key(Key::ArrowRight), &mut data, RECT, 1.0);
        state.on_key(&key(Key::Escape), &mut data, RECT, 1.0);
        assert_eq!(data, before);
        assert_eq!(state.pen_preview(), preview);
        assert_eq!(state.selection, selected);
        assert_eq!(state.tool(), MsegTool::SCurve);
        assert_eq!(
            state.on_mouse_down(x, y, &mut data, RECT, 1.0, false),
            Some(MsegEdit::Changed)
        );
        assert!(
            data.active()
                .iter()
                .any(|node| (node.time - 0.375).abs() < 1e-6)
        );
    }

    #[test]
    fn guide_wrap_scroll_and_painted_close_bounds_match_direct_entry_at_each_scale() {
        let mut tr = text();
        for curve_only in [false, true] {
            for scale in [0.5, 1.0, 2.0] {
                let mut state = MsegEditState::with_curve_only(curve_only);
                let mut data = MsegData::default();
                let before = data;
                let bounds = (900.0 * scale, 700.0 * scale);
                let rect = (10.0 * scale, 70.0 * scale, 880.0 * scale, 180.0 * scale);
                state.set_help_bounds(bounds);
                assert!(state.open_help(scale));
                let panel = state.help_layout(rect, scale).unwrap();
                assert!(panel.panel.3 > rect.3);
                let mut direct = tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
                crate::mseg::draw_mseg_dropdown(&mut direct, &mut tr, &state, rect);
                let mut expected =
                    tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
                state.draw_help(&mut expected, &mut tr, rect, scale);
                assert_eq!(direct.pixels(), expected.pixels());
                for _ in 0..4 {
                    for navigation in [Key::Home, Key::End] {
                        state.on_key(&key(navigation), &mut data, rect, scale);
                        let mut pm =
                            tiny_skia::Pixmap::new(bounds.0 as u32, bounds.1 as u32).unwrap();
                        state.draw_help(&mut pm, &mut tr, rect, scale);
                        let (x, y, w, h) = panel.panel;
                        for (i, pixel) in pm.pixels().iter().enumerate() {
                            if pixel.alpha() == 0 {
                                continue;
                            }
                            let px = (i as u32 % pm.width()) as f32;
                            let py = (i as u32 / pm.width()) as f32;
                            assert!(px >= x.floor() - 1.0 && px <= (x + w).ceil());
                            assert!(py >= y.floor() - 1.0 && py <= (y + h).ceil());
                        }
                    }
                    state.on_key(&key(Key::ArrowRight), &mut data, rect, scale);
                }
                state.on_mouse_down(
                    panel.close.0 + panel.close.2 * 0.5,
                    panel.close.1 + panel.close.3 * 0.5,
                    &mut data,
                    rect,
                    scale,
                    false,
                );
                assert!(!state.help_open());
                assert_eq!(data, before);
            }
        }
        for line in wrap(
            &mut tr,
            "Ctrl/Cmd+Alt+Shift+V preserves the selected phrase",
            12.0,
            70.0,
        ) {
            assert!(tr.text_width(&line, 12.0) <= 70.0);
        }
    }
}
