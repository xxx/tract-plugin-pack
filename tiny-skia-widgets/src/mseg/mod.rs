//! MSEG (multi-stage envelope generator) widget — core model, sampler, and
//! playback rules.
//!
//! `MsegData` is a fixed-capacity, `Copy`, heap-free envelope document: the
//! GUI edits it and a consuming plugin's audio thread reads it, and a `Copy`
//! `Vec`-free document crosses that boundary with a lock-free copy that never
//! allocates or frees on the audio thread.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

pub mod randomize;

#[allow(unused_imports)]
pub use randomize::*;

/// Maximum number of envelope nodes.
pub const MAX_NODES: usize = 128;

/// How playback behaves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum PlayMode {
    /// Runs once per trigger; honours sustain/loop while held.
    Triggered,
    /// Loops continuously; one MSEG span is one cycle.
    Cyclic,
}

/// How the envelope length is interpreted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum SyncMode {
    /// Length is `time_seconds`.
    Time,
    /// Length is `beats` (host tempo).
    Beat,
}

/// The hold behaviour — sustain point or loop region. Mutually exclusive.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum HoldMode {
    None,
    /// Triggered playback holds at this node index until released.
    Sustain(usize),
    /// Loop the `[start, end]` node-index range.
    Loop { start: usize, end: usize },
}

/// One envelope node. `tension`/`stepped` describe the segment FROM this node
/// to the next; the last active node's `tension`/`stepped` are unused.
#[derive(Clone, Copy, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MsegNode {
    /// 0..1 normalized phase position.
    pub time: f32,
    /// 0..1 normalized level.
    pub value: f32,
    /// -1..1 segment bow (concave/convex).
    pub tension: f32,
    /// Segment is an instant jump + flat hold.
    pub stepped: bool,
}

impl Default for MsegNode {
    fn default() -> Self {
        Self {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        }
    }
}

/// The editable, serializable envelope document. Fixed-capacity and `Copy`.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct MsegData {
    /// Storage for up to `MAX_NODES` nodes; only `nodes[..node_count]` are
    /// active, ordered strictly ascending by time.
    pub nodes: [MsegNode; MAX_NODES],
    pub node_count: usize,
    pub play_mode: PlayMode,
    pub hold: HoldMode,
    pub sync_mode: SyncMode,
    /// Active length when `sync_mode == Time`.
    pub time_seconds: f32,
    /// Active length when `sync_mode == Beat`.
    pub beats: f32,
    /// Horizontal grid: N divisions of the 0..1 span.
    pub time_divisions: u32,
    /// Vertical grid: N value levels.
    pub value_steps: u32,
    pub snap: bool,
}

impl Default for MsegData {
    /// A rising 0→1 ramp: two nodes, `Triggered`, `Time` sync, 1 s long.
    fn default() -> Self {
        let mut nodes = [MsegNode::default(); MAX_NODES];
        nodes[0] = MsegNode { time: 0.0, value: 0.0, tension: 0.0, stepped: false };
        nodes[1] = MsegNode { time: 1.0, value: 1.0, tension: 0.0, stepped: false };
        Self {
            nodes,
            node_count: 2,
            play_mode: PlayMode::Triggered,
            hold: HoldMode::None,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            beats: 1.0,
            time_divisions: 16,
            value_steps: 8,
            snap: true,
        }
    }
}

impl MsegData {
    /// The active nodes — `nodes[..node_count]`.
    pub fn active(&self) -> &[MsegNode] {
        &self.nodes[..self.node_count]
    }

    /// `true` iff the document satisfies every structural invariant. Slots
    /// `>= node_count` are not constrained.
    pub fn is_valid(&self) -> bool {
        if self.node_count < 2 || self.node_count > MAX_NODES {
            return false;
        }
        let a = self.active();
        if a[0].time != 0.0 || a[self.node_count - 1].time != 1.0 {
            return false;
        }
        for i in 0..self.node_count {
            let n = a[i];
            if !(0.0..=1.0).contains(&n.time)
                || !(0.0..=1.0).contains(&n.value)
                || !(-1.0..=1.0).contains(&n.tension)
            {
                return false;
            }
            if i > 0 && n.time <= a[i - 1].time {
                return false; // must be strictly ascending
            }
        }
        match self.hold {
            HoldMode::None => true,
            HoldMode::Sustain(i) => i < self.node_count,
            HoldMode::Loop { start, end } => start < end && end < self.node_count,
        }
    }

    /// Debug-only assertion of `is_valid`. No-op in release builds.
    pub fn debug_assert_valid(&self) {
        debug_assert!(self.is_valid(), "MsegData invariant violated: {self:?}");
    }
}

/// Shape factor: tension is scaled by this into the exponential warp exponent.
const TENSION_K: f32 = 5.0;

/// Warp a 0..1 segment-local position by `tension` (-1..1). `tension == 0`
/// is linear; positive bows slow-start (concave), negative fast-start.
/// Always maps 0->0 and 1->1.
pub fn warp(t: f32, tension: f32) -> f32 {
    if tension.abs() < 1e-6 {
        return t;
    }
    let k = tension * TENSION_K;
    ((k * t).exp() - 1.0) / (k.exp() - 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_a_two_node_ramp() {
        let d = MsegData::default();
        assert_eq!(d.node_count, 2);
        assert_eq!(d.active().len(), 2);
        assert_eq!(d.nodes[0].time, 0.0);
        assert_eq!(d.nodes[0].value, 0.0);
        assert_eq!(d.nodes[1].time, 1.0);
        assert_eq!(d.nodes[1].value, 1.0);
    }

    #[test]
    fn mseg_data_is_copy() {
        // Compile-time check: MsegData must be Copy (no heap) so it can cross
        // the GUI/audio boundary without allocating.
        fn assert_copy<T: Copy>() {}
        assert_copy::<MsegData>();
    }

    #[test]
    fn default_data_is_valid() {
        assert!(MsegData::default().is_valid());
    }

    #[test]
    fn unsorted_nodes_are_invalid() {
        let mut d = MsegData::default();
        d.node_count = 3;
        d.nodes[0] = MsegNode { time: 0.0, value: 0.0, ..Default::default() };
        d.nodes[1] = MsegNode { time: 0.8, value: 0.5, ..Default::default() };
        d.nodes[2] = MsegNode { time: 0.4, value: 1.0, ..Default::default() }; // out of order
        assert!(!d.is_valid());
    }

    #[test]
    fn endpoints_must_be_pinned() {
        let mut d = MsegData::default();
        d.nodes[0].time = 0.1; // first node must be at time 0.0
        assert!(!d.is_valid());
        let mut d = MsegData::default();
        d.nodes[1].time = 0.9; // last node must be at time 1.0
        assert!(!d.is_valid());
    }

    #[test]
    fn node_count_must_be_in_range() {
        let mut d = MsegData::default();
        d.node_count = 1;
        assert!(!d.is_valid());
        let mut d = MsegData::default();
        d.node_count = MAX_NODES + 1;
        assert!(!d.is_valid());
    }

    #[test]
    fn out_of_range_hold_index_is_invalid() {
        let mut d = MsegData::default(); // node_count == 2
        d.hold = HoldMode::Sustain(5);
        assert!(!d.is_valid());
        let mut d = MsegData::default();
        d.hold = HoldMode::Loop { start: 1, end: 0 }; // start > end
        assert!(!d.is_valid());
        let mut d = MsegData::default(); // node_count == 2, valid indices 0 and 1
        d.hold = HoldMode::Loop { start: 1, end: 1 }; // start == end is invalid
        assert!(!d.is_valid());
    }

    #[test]
    fn warp_zero_tension_is_linear() {
        assert!((warp(0.0, 0.0) - 0.0).abs() < 1e-6);
        assert!((warp(0.25, 0.0) - 0.25).abs() < 1e-6);
        assert!((warp(0.5, 0.0) - 0.5).abs() < 1e-6);
        assert!((warp(1.0, 0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn warp_pins_endpoints_for_any_tension() {
        for &k in &[-1.0, -0.5, 0.3, 1.0] {
            assert!((warp(0.0, k) - 0.0).abs() < 1e-5, "k={k}");
            assert!((warp(1.0, k) - 1.0).abs() < 1e-5, "k={k}");
        }
    }

    #[test]
    fn warp_is_monotonic() {
        for &k in &[-1.0, -0.4, 0.4, 1.0] {
            let mut prev = warp(0.0, k);
            for step in 1..=20 {
                let t = step as f32 / 20.0;
                let w = warp(t, k);
                assert!(w >= prev - 1e-5, "non-monotonic at t={t}, k={k}");
                prev = w;
            }
        }
    }

    #[test]
    fn warp_bows_in_opposite_directions() {
        // Positive tension: slow start -> midpoint output below 0.5.
        // Negative tension: fast start -> midpoint output above 0.5.
        assert!(warp(0.5, 1.0) < 0.5);
        assert!(warp(0.5, -1.0) > 0.5);
    }
}
