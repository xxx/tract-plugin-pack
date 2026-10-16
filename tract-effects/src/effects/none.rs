use super::{Effect, ParamSpec};

/// A silent "no-effect" — used when a track has no effect assigned. The row
/// still occupies a lane and the propagation engine still lights its cells,
/// but the lane contributes nothing to the wet sum (an unassigned track has
/// no audio to forward). Declares no modulatable parameters.
pub struct NoneEffect;

impl NoneEffect {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NoneEffect {
    fn default() -> Self {
        Self::new()
    }
}

impl Effect for NoneEffect {
    fn process_sample(&mut self, _left: f32, _right: f32) -> (f32, f32) {
        (0.0, 0.0)
    }

    fn set_sample_rate(&mut self, _sample_rate: f32) {}

    fn reset(&mut self) {}

    fn parameters(&self) -> &'static [ParamSpec] {
        &[]
    }

    fn set_param(&mut self, _index: usize, _value: f32) {}
}
