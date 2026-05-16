//! MSEG (multi-stage envelope generator) widget — core model, sampler, and
//! playback rules.
//!
//! `MsegData` is a fixed-capacity, `Copy`, heap-free envelope document: the
//! GUI edits it and a consuming plugin's audio thread reads it, and a `Copy`
//! `Vec`-free document crosses that boundary with a lock-free copy that never
//! allocates or frees on the audio thread.
//!
//! See `docs/superpowers/specs/2026-05-16-mseg-editor-widget-design.md`.

pub mod editor;
pub mod randomize;
pub mod render;

pub use editor::*;
pub use randomize::*;
#[allow(unused_imports)] // render.rs is a stub; exports are wired up in later tasks
pub use render::*;

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
    Loop {
        start: usize,
        end: usize,
    },
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
        nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        nodes[1] = MsegNode {
            time: 1.0,
            value: 1.0,
            tension: 0.0,
            stepped: false,
        };
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
    ///
    /// `MIN_NODE_GAP` is an *insertion* constraint enforced by the node
    /// mutation operations, NOT a document invariant: `is_valid` requires only
    /// strictly-ascending times, so a (de)serialized document may legitimately
    /// have neighbouring nodes closer together than `MIN_NODE_GAP`.
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

    /// Smallest time gap allowed between adjacent nodes.
    const MIN_NODE_GAP: f32 = 1e-4;

    /// Insert a node at `(time, value)`, keeping time order. `time`/`value`
    /// are clamped to 0..1; `time` is clamped strictly between the existing
    /// nodes it falls between. Returns the new node's index, or `None` if the
    /// document is already at `MAX_NODES`. The new node's segment is linear
    /// (tension 0, not stepped).
    pub fn insert_node(&mut self, time: f32, value: f32) -> Option<usize> {
        if self.node_count >= MAX_NODES {
            return None;
        }
        let value = value.clamp(0.0, 1.0);
        // Insertion index: first active node with a strictly greater time.
        let k = self.nodes[..self.node_count]
            .iter()
            .position(|n| n.time > time)
            .unwrap_or(self.node_count);
        // Endpoints stay pinned: never insert before index 1 or after the last.
        let k = k.clamp(1, self.node_count - 1);
        let lo = self.nodes[k - 1].time + Self::MIN_NODE_GAP;
        let hi = self.nodes[k].time - Self::MIN_NODE_GAP;
        // The two neighbours are closer than 2*MIN_NODE_GAP — no room for a
        // node between them that honours the gap on both sides.
        if lo > hi {
            return None;
        }
        let time = time.clamp(lo, hi);
        // Shift [k..node_count) up by one.
        let mut i = self.node_count;
        while i > k {
            self.nodes[i] = self.nodes[i - 1];
            i -= 1;
        }
        self.nodes[k] = MsegNode {
            time,
            value,
            tension: 0.0,
            stepped: false,
        };
        self.node_count += 1;
        self.shift_hold_for_insert(k);
        self.debug_assert_valid();
        Some(k)
    }

    /// Remove the node at `idx`. Endpoints (index 0 and the last) cannot be
    /// removed. Returns `true` if a node was removed.
    pub fn remove_node(&mut self, idx: usize) -> bool {
        if idx == 0 || idx + 1 >= self.node_count {
            return false;
        }
        // Shift (idx, node_count) down by one.
        for i in idx..self.node_count - 1 {
            self.nodes[i] = self.nodes[i + 1];
        }
        self.node_count -= 1;
        self.fix_hold_for_remove(idx);
        self.debug_assert_valid();
        true
    }

    /// Move the node at `idx` to `(time, value)`. `value` is clamped to 0..1.
    /// Endpoint nodes keep their pinned time (0.0 / 1.0); interior nodes have
    /// `time` clamped strictly between their neighbours.
    ///
    /// When the two neighbours are already closer than `2 * MIN_NODE_GAP`,
    /// the moved node still stays strictly between them, but the
    /// `MIN_NODE_GAP` clearance is then best-effort.
    pub fn move_node(&mut self, idx: usize, time: f32, value: f32) {
        if idx >= self.node_count {
            return;
        }
        self.nodes[idx].value = value.clamp(0.0, 1.0);
        let is_endpoint = idx == 0 || idx + 1 == self.node_count;
        if !is_endpoint {
            let lo = self.nodes[idx - 1].time + Self::MIN_NODE_GAP;
            let hi = self.nodes[idx + 1].time - Self::MIN_NODE_GAP;
            self.nodes[idx].time = time.clamp(lo.min(hi), lo.max(hi));
        }
        self.debug_assert_valid();
    }

    /// After inserting a node at index `k`, bump every `hold` index `>= k`.
    fn shift_hold_for_insert(&mut self, k: usize) {
        let bump = |i: usize| if i >= k { i + 1 } else { i };
        self.hold = match self.hold {
            HoldMode::None => HoldMode::None,
            HoldMode::Sustain(i) => HoldMode::Sustain(bump(i)),
            HoldMode::Loop { start, end } => HoldMode::Loop {
                start: bump(start),
                end: bump(end),
            },
        };
    }

    /// After removing the node at `idx`, fix up `hold`: a reference to the
    /// removed node clears `hold`; higher indices shift down.
    fn fix_hold_for_remove(&mut self, idx: usize) {
        let adjust = |i: usize| -> Option<usize> {
            match i.cmp(&idx) {
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some(i - 1),
                std::cmp::Ordering::Less => Some(i),
            }
        };
        self.hold = match self.hold {
            HoldMode::None => HoldMode::None,
            HoldMode::Sustain(i) => match adjust(i) {
                Some(i) => HoldMode::Sustain(i),
                None => HoldMode::None,
            },
            HoldMode::Loop { start, end } => match (adjust(start), adjust(end)) {
                (Some(s), Some(e)) if s < e => HoldMode::Loop { start: s, end: e },
                _ => HoldMode::None,
            },
        };
    }
}

/// Compact, `Vec`-backed mirror of `MsegData` used only for (de)serialization.
/// Lives on the GUI thread at load/save time — never on the audio thread.
#[derive(serde::Serialize, serde::Deserialize)]
struct MsegDataSerde {
    nodes: Vec<MsegNode>,
    play_mode: PlayMode,
    hold: HoldMode,
    sync_mode: SyncMode,
    time_seconds: f32,
    beats: f32,
    time_divisions: u32,
    value_steps: u32,
    snap: bool,
}

impl From<&MsegData> for MsegDataSerde {
    fn from(d: &MsegData) -> Self {
        Self {
            nodes: d.active().to_vec(),
            play_mode: d.play_mode,
            hold: d.hold,
            sync_mode: d.sync_mode,
            time_seconds: d.time_seconds,
            beats: d.beats,
            time_divisions: d.time_divisions,
            value_steps: d.value_steps,
            snap: d.snap,
        }
    }
}

impl serde::Serialize for MsegData {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        MsegDataSerde::from(self).serialize(s)
    }
}

impl<'de> serde::Deserialize<'de> for MsegData {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = MsegDataSerde::deserialize(d)?;
        let mut data = MsegData {
            nodes: [MsegNode::default(); MAX_NODES],
            node_count: raw.nodes.len().min(MAX_NODES),
            play_mode: raw.play_mode,
            hold: raw.hold,
            sync_mode: raw.sync_mode,
            time_seconds: raw.time_seconds,
            beats: raw.beats,
            time_divisions: raw.time_divisions,
            value_steps: raw.value_steps,
            snap: raw.snap,
        };
        for (i, n) in raw.nodes.iter().take(MAX_NODES).enumerate() {
            data.nodes[i] = *n;
        }
        // A corrupt or hand-edited blob must not yield an invalid document.
        if !data.is_valid() {
            return Err(serde::de::Error::custom("invalid MsegData"));
        }
        Ok(data)
    }
}

/// Sample the envelope's raw shape at `phase` (0..1, clamped). Pure — used by
/// both rendering and a consuming plugin's DSP.
pub fn value_at_phase(data: &MsegData, phase: f32) -> f32 {
    let a = data.active();
    let phase = phase.clamp(0.0, 1.0);

    // At or past the last node -> its value.
    if phase >= a[data.node_count - 1].time {
        return a[data.node_count - 1].value;
    }
    // Find the segment: the last node whose time is <= phase.
    let mut i = 0;
    for (k, node) in a.iter().enumerate().take(data.node_count - 1) {
        if node.time <= phase {
            i = k;
        } else {
            break;
        }
    }
    let n0 = a[i];
    let n1 = a[i + 1];
    if n0.stepped {
        return n0.value;
    }
    let span = n1.time - n0.time;
    let t = if span > 1e-9 {
        (phase - n0.time) / span
    } else {
        0.0
    };
    n0.value + (n1.value - n0.value) * warp(t, n0.tension)
}

/// Wrap `p` into the half-open range `[lo, hi)`.
fn wrap_into(p: f32, lo: f32, hi: f32) -> f32 {
    let span = (hi - lo).max(1e-9);
    lo + (p - lo).rem_euclid(span)
}

/// Advance the playhead one step, applying the document's playback rules.
/// Returns `(next_phase, finished)`. `finished` is only ever `true` in
/// triggered playback once the playhead reaches the end. Pure — the consuming
/// plugin owns the `phase` value and the `released` flag.
pub fn advance(data: &MsegData, phase: f32, dt: f32, released: bool) -> (f32, bool) {
    let a = data.active();
    match data.play_mode {
        PlayMode::Cyclic => {
            let (lo, hi) = match data.hold {
                HoldMode::Loop { start, end } => (a[start].time, a[end].time),
                _ => (0.0, 1.0),
            };
            (wrap_into(phase + dt, lo, hi), false)
        }
        PlayMode::Triggered => {
            let mut p = phase + dt;
            if !released {
                match data.hold {
                    HoldMode::Sustain(i) => {
                        let st = a[i].time;
                        if p > st {
                            p = st;
                        }
                        return (p, false);
                    }
                    HoldMode::Loop { start, end } => {
                        let (lo, hi) = (a[start].time, a[end].time);
                        if p >= hi {
                            p = wrap_into(p, lo, hi);
                        }
                        return (p, false);
                    }
                    HoldMode::None => {}
                }
            }
            if p >= 1.0 {
                (1.0, true)
            } else {
                (p, false)
            }
        }
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
        d.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            ..Default::default()
        };
        d.nodes[1] = MsegNode {
            time: 0.8,
            value: 0.5,
            ..Default::default()
        };
        d.nodes[2] = MsegNode {
            time: 0.4,
            value: 1.0,
            ..Default::default()
        }; // out of order
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

    #[test]
    fn value_at_phase_linear_ramp() {
        let d = MsegData::default(); // 0->1 linear ramp
        assert!((value_at_phase(&d, 0.0) - 0.0).abs() < 1e-6);
        assert!((value_at_phase(&d, 0.5) - 0.5).abs() < 1e-6);
        assert!((value_at_phase(&d, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn value_at_phase_clamps_out_of_range() {
        let d = MsegData::default();
        assert!((value_at_phase(&d, -0.5) - 0.0).abs() < 1e-6);
        assert!((value_at_phase(&d, 2.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn value_at_phase_stepped_segment_holds() {
        let mut d = MsegData::default();
        d.nodes[0].stepped = true; // segment 0 holds nodes[0].value (0.0)
        assert!((value_at_phase(&d, 0.0) - 0.0).abs() < 1e-6);
        assert!((value_at_phase(&d, 0.99) - 0.0).abs() < 1e-6); // still held
        assert!((value_at_phase(&d, 1.0) - 1.0).abs() < 1e-6); // last node value
    }

    #[test]
    fn value_at_phase_respects_tension() {
        let mut d = MsegData::default();
        d.nodes[0].tension = 1.0; // slow-start bow
                                  // Midpoint output should sit below the linear 0.5.
        assert!(value_at_phase(&d, 0.5) < 0.5);
    }

    #[test]
    fn value_at_phase_three_nodes() {
        let mut d = MsegData::default();
        d.node_count = 3;
        d.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[1] = MsegNode {
            time: 0.5,
            value: 1.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[2] = MsegNode {
            time: 1.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        assert!((value_at_phase(&d, 0.25) - 0.5).abs() < 1e-6); // up-ramp midpoint
        assert!((value_at_phase(&d, 0.5) - 1.0).abs() < 1e-6); // peak
        assert!((value_at_phase(&d, 0.75) - 0.5).abs() < 1e-6); // down-ramp midpoint
    }

    #[test]
    fn advance_triggered_runs_to_end_and_finishes() {
        let d = MsegData::default(); // Triggered, hold None
        let (p, finished) = advance(&d, 0.5, 0.25, false);
        assert!((p - 0.75).abs() < 1e-6);
        assert!(!finished);
        let (p, finished) = advance(&d, 0.9, 0.25, false);
        assert!((p - 1.0).abs() < 1e-6);
        assert!(finished);
    }

    #[test]
    fn advance_cyclic_wraps() {
        let mut d = MsegData::default();
        d.play_mode = PlayMode::Cyclic;
        let (p, finished) = advance(&d, 0.9, 0.25, false);
        assert!(!finished);
        assert!((p - 0.15).abs() < 1e-6, "0.9 + 0.25 wraps to 0.15, got {p}");
    }

    #[test]
    fn advance_sustain_holds_until_released() {
        let mut d = MsegData::default();
        d.node_count = 3;
        d.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[1] = MsegNode {
            time: 0.5,
            value: 1.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[2] = MsegNode {
            time: 1.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.hold = HoldMode::Sustain(1); // node 1 is at time 0.5
                                       // Held: phase cannot pass the sustain node's time.
        let (p, finished) = advance(&d, 0.45, 0.25, false);
        assert!((p - 0.5).abs() < 1e-6, "held at sustain time, got {p}");
        assert!(!finished);
        // Released: advances past the sustain point normally.
        let (p, finished) = advance(&d, 0.5, 0.25, true);
        assert!((p - 0.75).abs() < 1e-6);
        assert!(!finished);
    }

    #[test]
    fn advance_loop_wraps_while_held_then_exits_on_release() {
        let mut d = MsegData::default();
        d.node_count = 3;
        d.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[1] = MsegNode {
            time: 0.25,
            value: 1.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[2] = MsegNode {
            time: 1.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.hold = HoldMode::Loop { start: 0, end: 1 }; // loop [0.0, 0.25]
                                                      // Held: crossing the loop end wraps back toward the loop start.
        let (p, _) = advance(&d, 0.2, 0.1, false);
        assert!(p < 0.25 && p >= 0.0, "looped back into [0,0.25], got {p}");
        // Released: advances past the loop end toward the real end.
        let (p, _) = advance(&d, 0.2, 0.1, true);
        assert!((p - 0.3).abs() < 1e-6, "released advances freely, got {p}");
    }

    // --- Task 6: node mutation tests ---

    #[test]
    fn insert_node_keeps_time_order() {
        let mut d = MsegData::default(); // nodes at 0.0 and 1.0
        let idx = d.insert_node(0.5, 0.7).unwrap();
        assert_eq!(idx, 1);
        assert_eq!(d.node_count, 3);
        assert_eq!(d.nodes[1].time, 0.5);
        assert_eq!(d.nodes[1].value, 0.7);
        assert!(d.is_valid());
    }

    #[test]
    fn insert_node_refuses_at_capacity() {
        let mut d = MsegData::default();
        while d.node_count < MAX_NODES {
            // Spread fill: every gap is ~0.99/MAX_NODES wide, far wider than
            // 2*MIN_NODE_GAP, so the fill never trips the no-room path.
            let t = d.node_count as f32 / MAX_NODES as f32 * 0.99;
            d.insert_node(t, 0.5);
        }
        assert_eq!(d.node_count, MAX_NODES);
        assert!(d.insert_node(0.999, 0.5).is_none());
    }

    #[test]
    fn insert_node_refuses_when_no_room() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5).unwrap(); // nodes: 0.0, 0.5, 1.0
                                          // Second insert at 0.5 clamps to 0.5 + MIN_NODE_GAP, leaving nodes 1 and 2
                                          // exactly MIN_NODE_GAP apart.
        d.insert_node(0.5, 0.5).unwrap();
        let before = d.node_count;
        // No gap-respecting room between those two nodes -> refused, not a panic.
        assert!(d.insert_node(0.50005, 0.5).is_none());
        assert_eq!(d.node_count, before);
        assert!(d.is_valid());
    }

    #[test]
    fn insert_node_shifts_hold_indices() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5); // node_count 3
        d.hold = HoldMode::Sustain(2); // the 1.0 endpoint
        d.insert_node(0.25, 0.5); // inserts at index 1, pushing others up
        assert_eq!(d.hold, HoldMode::Sustain(3));
        assert!(d.is_valid());
    }

    #[test]
    fn remove_node_refuses_endpoints() {
        let mut d = MsegData::default();
        assert!(!d.remove_node(0));
        assert!(!d.remove_node(1)); // last node
        assert_eq!(d.node_count, 2);
    }

    #[test]
    fn remove_node_deletes_interior_and_fixes_hold() {
        let mut d = MsegData::default();
        d.insert_node(0.3, 0.5); // idx 1
        d.insert_node(0.6, 0.5); // idx 2
        d.hold = HoldMode::Sustain(2);
        assert!(d.remove_node(1)); // remove the 0.3 node
        assert_eq!(d.node_count, 3);
        assert_eq!(d.hold, HoldMode::Sustain(1)); // shifted down
        assert!(d.is_valid());
    }

    #[test]
    fn remove_node_clears_hold_referencing_removed_node() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5);
        d.hold = HoldMode::Sustain(1);
        assert!(d.remove_node(1));
        assert_eq!(d.hold, HoldMode::None);
    }

    #[test]
    fn move_node_clamps_interior_between_neighbors() {
        let mut d = MsegData::default();
        d.insert_node(0.5, 0.5); // idx 1
        d.move_node(1, 5.0, 2.0); // wildly out of range
        assert!(d.nodes[1].time > 0.0 && d.nodes[1].time < 1.0);
        assert!(d.nodes[1].value >= 0.0 && d.nodes[1].value <= 1.0);
        assert!(d.is_valid());
    }

    #[test]
    fn move_node_pins_endpoint_time() {
        let mut d = MsegData::default();
        d.move_node(0, 0.4, 0.8); // try to move the first node's time
        assert_eq!(d.nodes[0].time, 0.0); // time pinned
        assert_eq!(d.nodes[0].value, 0.8); // value moved
        d.move_node(1, 0.4, 0.2);
        assert_eq!(d.nodes[1].time, 1.0); // time pinned
        assert_eq!(d.nodes[1].value, 0.2);
    }

    // --- Task 8: serde persistence tests ---

    #[test]
    fn mseg_data_json_round_trips() {
        let mut d = MsegData::default();
        d.insert_node(0.3, 0.7);
        d.insert_node(0.6, 0.2);
        d.nodes[1].tension = 0.5;
        d.nodes[2].stepped = true;
        d.hold = HoldMode::Sustain(2);
        d.play_mode = PlayMode::Cyclic;
        d.sync_mode = SyncMode::Beat;
        d.beats = 2.0;
        d.time_divisions = 24;
        d.value_steps = 6;
        d.snap = false;

        let json = serde_json::to_string(&d).unwrap();
        let back: MsegData = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
        assert!(back.is_valid());
    }

    #[test]
    fn mseg_data_json_is_compact() {
        // The default 2-node document must not serialize all MAX_NODES slots.
        let json = serde_json::to_string(&MsegData::default()).unwrap();
        assert!(
            json.len() < 400,
            "serialized default unexpectedly large ({} bytes)",
            json.len()
        );
    }

    #[test]
    fn mseg_data_json_rejects_invalid_blob() {
        // Serialize a deliberately-invalid document (zero active nodes), then
        // confirm deserialization rejects it instead of yielding a bad MsegData.
        let mut d = MsegData::default();
        d.node_count = 0; // invalid: a valid document needs >= 2 nodes
        let json = serde_json::to_string(&d).unwrap();
        let result: Result<MsegData, _> = serde_json::from_str(&json);
        assert!(
            result.is_err(),
            "invalid blob must be rejected, got {result:?}"
        );
    }

    #[test]
    fn randomized_data_serde_round_trips() {
        // A randomized document (up to MAX_NODES stepped nodes) must survive the
        // compact serde round-trip unchanged.
        let mut d = MsegData::default();
        randomize(&mut d, RandomStyle::Stepped, 999);
        let json = serde_json::to_string(&d).unwrap();
        let back: MsegData = serde_json::from_str(&json).unwrap();
        assert_eq!(d, back);
        assert!(back.is_valid());
    }

    #[test]
    fn advance_sustain_steady_state_holds() {
        // phase already AT the sustain point and still held: it must stay clamped
        // there, not drift forward.
        let mut d = MsegData::default();
        d.node_count = 3;
        d.nodes[0] = MsegNode {
            time: 0.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[1] = MsegNode {
            time: 0.5,
            value: 1.0,
            tension: 0.0,
            stepped: false,
        };
        d.nodes[2] = MsegNode {
            time: 1.0,
            value: 0.0,
            tension: 0.0,
            stepped: false,
        };
        d.hold = HoldMode::Sustain(1); // sustain at time 0.5
        let (p, finished) = advance(&d, 0.5, 0.1, false);
        assert!((p - 0.5).abs() < 1e-6, "steady-state hold, got {p}");
        assert!(!finished);
    }
}
