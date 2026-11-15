//! GUI-side reusable curves, independent of playback and routing.

use super::{HoldMode, MAX_NODES, MsegData, MsegNode};

pub const MAX_SHAPE_BYTES: usize = 65_536;

#[derive(Clone, Debug, PartialEq)]
pub struct CurveShape {
    pub(crate) nodes: Vec<MsegNode>,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ShapeFile {
    version: u32,
    nodes: Vec<MsegNode>,
}

impl CurveShape {
    pub fn capture(data: &MsegData) -> Self {
        Self {
            nodes: data.active().to_vec(),
        }
    }

    pub fn nodes(&self) -> &[MsegNode] {
        &self.nodes
    }

    pub fn from_json(text: &str) -> Result<Self, ShapeError> {
        if text.len() > MAX_SHAPE_BYTES {
            return Err(ShapeError::Invalid);
        }
        let file: ShapeFile = serde_json::from_str(text).map_err(|_| ShapeError::Invalid)?;
        if file.version != 1 {
            return Err(ShapeError::Invalid);
        }
        validate_nodes(&file.nodes)?;
        Ok(Self { nodes: file.nodes })
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&ShapeFile {
            version: 1,
            nodes: self.nodes.clone(),
        })
    }

    pub fn apply(&self, data: &mut MsegData, fit_height: bool) -> Result<bool, ShapeError> {
        let mut nodes = self.nodes.clone();
        if fit_height {
            let (lo, hi) = value_range(data.active());
            let (src_lo, src_hi) = value_range(&nodes);
            for node in &mut nodes {
                node.value = if src_hi > src_lo {
                    lo + (node.value - src_lo) / (src_hi - src_lo) * (hi - lo)
                } else {
                    (lo + hi) * 0.5
                };
            }
        }
        validate_nodes(&nodes)?;
        let changed = data.active() != nodes || data.hold != HoldMode::None;
        data.nodes[..nodes.len()].copy_from_slice(&nodes);
        data.node_count = nodes.len();
        data.hold = HoldMode::None;
        Ok(changed)
    }
}

pub(crate) fn validate_nodes(nodes: &[MsegNode]) -> Result<(), ShapeError> {
    if nodes.len() > MAX_NODES {
        return Err(ShapeError::Capacity);
    }
    if nodes.len() < 2 || nodes[0].time != 0.0 || nodes[nodes.len() - 1].time != 1.0 {
        return Err(ShapeError::Invalid);
    }
    if nodes.iter().any(|n| {
        !(0.0..=1.0).contains(&n.time)
            || !(0.0..=1.0).contains(&n.value)
            || !(-1.0..=1.0).contains(&n.tension)
    }) {
        return Err(ShapeError::Invalid);
    }
    if nodes.windows(2).any(|pair| pair[0].time >= pair[1].time) {
        return Err(ShapeError::Collision);
    }
    Ok(())
}

pub(crate) fn value_range(nodes: &[MsegNode]) -> (f32, f32) {
    nodes.iter().fold((1.0_f32, 0.0_f32), |(lo, hi), n| {
        (lo.min(n.value), hi.max(n.value))
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FactoryShape {
    FlatHigh,
    FlatCenter,
    RampUp,
    RampDown,
    Triangle,
    Sine,
    Square,
    Duck,
}

impl FactoryShape {
    pub const ALL: [Self; 8] = [
        Self::FlatHigh,
        Self::FlatCenter,
        Self::RampUp,
        Self::RampDown,
        Self::Triangle,
        Self::Sine,
        Self::Square,
        Self::Duck,
    ];
    pub const LABELS: [&'static str; 8] = [
        "Flat high",
        "Flat center",
        "Ramp up",
        "Ramp down",
        "Triangle",
        "Sine",
        "Square",
        "Duck",
    ];
}

pub fn factory_shape(kind: FactoryShape) -> CurveShape {
    let mut nodes: Vec<MsegNode> = match kind {
        FactoryShape::FlatHigh => vec![node(0.0, 1.0), node(1.0, 1.0)],
        FactoryShape::FlatCenter => vec![node(0.0, 0.5), node(1.0, 0.5)],
        FactoryShape::RampUp => vec![node(0.0, 0.0), node(1.0, 1.0)],
        FactoryShape::RampDown => vec![node(0.0, 1.0), node(1.0, 0.0)],
        FactoryShape::Triangle => vec![node(0.0, 0.0), node(0.5, 1.0), node(1.0, 0.0)],
        FactoryShape::Sine => (0..=32)
            .map(|i| {
                let t = i as f32 / 32.0;
                node(t, 0.5 - 0.5 * (t * std::f32::consts::TAU).cos())
            })
            .collect(),
        FactoryShape::Square => vec![node(0.0, 1.0), node(0.5, 0.0), node(1.0, 1.0)],
        FactoryShape::Duck => vec![
            node(0.0, 0.0),
            node(0.12, 0.0),
            node(0.6, 1.0),
            node(1.0, 1.0),
        ],
    };
    if kind == FactoryShape::Square {
        for n in &mut nodes {
            n.stepped = true;
        }
    } else if kind == FactoryShape::Duck {
        nodes[1].tension = -0.65;
    }
    CurveShape { nodes }
}

pub(crate) fn node(time: f32, value: f32) -> MsegNode {
    MsegNode {
        time,
        value,
        tension: 0.0,
        stepped: false,
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum MsegTool {
    #[default]
    Pointer,
    Line,
    Arc,
    SCurve,
}

impl MsegTool {
    pub const ALL: [Self; 4] = [Self::Pointer, Self::Line, Self::Arc, Self::SCurve];
    pub const LABELS: [&'static str; 4] = ["Pointer", "Line pen", "Arc pen", "S-curve pen"];
    pub fn label(self) -> &'static str {
        Self::LABELS[Self::ALL.iter().position(|t| *t == self).unwrap_or(0)]
    }
}

pub(crate) fn pen_nodes(tool: MsegTool, start: (f32, f32), end: (f32, f32)) -> Vec<MsegNode> {
    let (left, right) = if start.0 <= end.0 {
        (start, end)
    } else {
        (end, start)
    };
    let segments = if tool == MsegTool::SCurve { 16 } else { 1 };
    let mut nodes: Vec<_> = (0..=segments)
        .map(|i| {
            let t = i as f32 / segments as f32;
            let level = if tool == MsegTool::SCurve {
                t * t * (3.0 - 2.0 * t)
            } else {
                t
            };
            node(
                left.0 + (right.0 - left.0) * t,
                left.1 + (right.1 - left.1) * level,
            )
        })
        .collect();
    if tool == MsegTool::Arc {
        nodes[0].tension = if start.0 <= end.0 { -0.65 } else { 0.65 };
    }
    nodes
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShapeError {
    Invalid,
    Capacity,
    Selection,
    Collision,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid curve data",
            Self::Capacity => "This edit would exceed 128 points",
            Self::Selection => "Select adjacent points for this edit",
            Self::Collision => "This edit would overlap points",
        })
    }
}

impl std::error::Error for ShapeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{HoldMode, PlayMode, Polarity, SyncMode};

    #[test]
    fn applying_a_shape_preserves_the_destinations_settings() {
        let shape = CurveShape::capture(&MsegData::default());
        let mut data = MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Beat,
            beats: 8.0,
            time_seconds: 5.0,
            polarity: Polarity::Bipolar,
            time_divisions: 12,
            value_steps: 7,
            snap: false,
            hold: HoldMode::Sustain(1),
            ..MsegData::default()
        };
        let settings = data;
        shape.apply(&mut data, false).unwrap();
        assert_eq!(data.active(), shape.nodes());
        assert_eq!(data.hold, HoldMode::None);
        data.hold = settings.hold;
        assert_eq!(data, settings);
    }

    #[test]
    fn fitting_a_shape_uses_the_destinations_value_range() {
        let mut data = MsegData::default();
        data.nodes[0].value = 0.2;
        data.nodes[1].value = 0.6;
        factory_shape(FactoryShape::Triangle)
            .apply(&mut data, true)
            .unwrap();
        assert!((data.nodes[0].value - 0.2).abs() < 1e-6);
        assert!((data.nodes[1].value - 0.6).abs() < 1e-6);
        assert!((data.nodes[2].value - 0.2).abs() < 1e-6);
    }

    #[test]
    fn shape_json_roundtrips_and_rejects_invalid_documents() {
        let shape = factory_shape(FactoryShape::Sine);
        assert_eq!(
            CurveShape::from_json(&shape.to_json().unwrap()).unwrap(),
            shape
        );
        for text in [
            r#"{"version":2,"nodes":[]}"#,
            r#"{"version":1,"nodes":[]}"#,
            r#"{"version":1,"nodes":[{"time":0,"value":2,"tension":0,"stepped":false},{"time":1,"value":1,"tension":0,"stepped":false}]}"#,
            r#"{"version":1,"nodes":[{"time":0,"value":0,"tension":0,"stepped":false},{"time":0,"value":1,"tension":0,"stepped":false}]}"#,
        ] {
            assert!(CurveShape::from_json(text).is_err(), "accepted {text}");
        }
        let oversized = " ".repeat(65_537);
        assert!(CurveShape::from_json(&oversized).is_err());
    }
}
