//! Transient editor coordinates; never part of the envelope document.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MsegViewport {
    pub(crate) time: (f32, f32),
    pub(crate) value: (f32, f32),
}

impl Default for MsegViewport {
    fn default() -> Self {
        Self {
            time: (0.0, 1.0),
            value: (0.0, 1.0),
        }
    }
}

impl MsegViewport {
    pub const MIN_SPAN: f32 = 1.0 / 64.0;

    pub fn time_range(self) -> (f32, f32) {
        self.time
    }
    pub fn value_range(self) -> (f32, f32) {
        self.value
    }
    pub fn contains(self, time: f32, value: f32) -> bool {
        time >= self.time.0 && time <= self.time.1 && value >= self.value.0 && value <= self.value.1
    }
    pub fn phase_at(self, fraction: f32) -> f32 {
        self.time.0 + fraction * (self.time.1 - self.time.0)
    }
    pub fn level_at(self, fraction: f32) -> f32 {
        self.value.0 + fraction * (self.value.1 - self.value.0)
    }

    fn bounded(start: f32, span: f32) -> (f32, f32) {
        let span = span.clamp(Self::MIN_SPAN, 1.0);
        let lo = start.clamp(0.0, 1.0 - span);
        (lo, lo + span)
    }

    pub fn zoom(&mut self, amount: f32, time_fraction: f32, value_fraction: f32, vertical: bool) {
        if !amount.is_finite() {
            return;
        }
        let axis = if vertical {
            &mut self.value
        } else {
            &mut self.time
        };
        let fraction = if vertical {
            value_fraction
        } else {
            time_fraction
        }
        .clamp(0.0, 1.0);
        let span = axis.1 - axis.0;
        let anchor = axis.0 + span * fraction;
        let next = (span * (-amount.clamp(-32.0, 32.0) * 0.15).exp()).clamp(Self::MIN_SPAN, 1.0);
        *axis = Self::bounded(anchor - next * fraction, next);
    }

    pub fn pan_from(&mut self, start: Self, delta_time: f32, delta_value: f32) {
        self.time = Self::bounded(
            start.time.0 + delta_time * (start.time.1 - start.time.0),
            start.time.1 - start.time.0,
        );
        self.value = Self::bounded(
            start.value.0 + delta_value * (start.value.1 - start.value.0),
            start.value.1 - start.value.0,
        );
    }

    pub fn fit(&mut self, time: (f32, f32), value: (f32, f32)) {
        let fit_axis = |(lo, hi): (f32, f32)| {
            let span = ((hi - lo) * 1.1).clamp(Self::MIN_SPAN, 1.0);
            Self::bounded((lo + hi - span) * 0.5, span)
        };
        self.time = fit_axis(time);
        self.value = fit_axis(value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zoom_preserves_the_pointer_anchor_and_the_other_axis() {
        let mut view = MsegViewport::default();
        view.zoom(4.0, 0.3, 0.7, false);
        assert!((view.phase_at(0.3) - 0.3).abs() < 1e-6);
        assert_eq!(view.value, (0.0, 1.0));
        view.zoom(4.0, 0.3, 0.7, true);
        assert!((view.level_at(0.7) - 0.7).abs() < 1e-6);
    }

    #[test]
    fn zoom_pan_and_fit_stay_bounded_and_have_a_minimum_span() {
        let mut view = MsegViewport::default();
        for _ in 0..100 {
            view.zoom(32.0, 0.5, 0.5, false);
        }
        assert!((view.time.1 - view.time.0 - MsegViewport::MIN_SPAN).abs() < 1e-6);
        view.pan_from(view, 1000.0, -1000.0);
        assert_eq!(view.time.1, 1.0);
        view.fit((0.4, 0.4), (0.1, 0.9));
        assert!(view.contains(0.4, 0.1) && view.contains(0.4, 0.9));
        for _ in 0..100 {
            view.zoom(-32.0, 0.5, 0.5, false);
        }
        assert_eq!(view.time, (0.0, 1.0));
    }
}
