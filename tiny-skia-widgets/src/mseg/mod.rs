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
}
