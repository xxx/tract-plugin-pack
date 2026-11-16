//! GUI-side reusable curves, independent of playback and routing.

use super::{HoldMode, MAX_NODES, MsegData, MsegNode, SegmentShape};

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
        if !matches!(file.version, 1 | 2)
            || (file.version == 1
                && file
                    .nodes
                    .iter()
                    .any(|n| !n.shape.is_curve() || !n.weight.is_hard()))
        {
            return Err(ShapeError::Invalid);
        }
        validate_nodes(&file.nodes)?;
        Ok(Self { nodes: file.nodes })
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&ShapeFile {
            version: if self
                .nodes
                .iter()
                .all(|n| n.shape.is_curve() && n.weight.is_hard())
            {
                1
            } else {
                2
            },
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
        ..MsegNode::default()
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
    let mut nodes = vec![node(left.0, left.1), node(right.0, right.1)];
    if tool == MsegTool::SCurve {
        nodes[0].shape = SegmentShape::SCurve;
    }
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
    UnavailableGrid,
}

impl std::fmt::Display for ShapeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid curve data",
            Self::Capacity => "This edit would exceed 128 points",
            Self::Selection => "Select adjacent points for this edit",
            Self::Collision => "This edit would overlap points",
            Self::UnavailableGrid => "Semitone grid needs a pitched target with reachable notes",
        })
    }
}

impl std::error::Error for ShapeError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{HoldMode, PlayMode, PointWeight, Polarity, SyncMode};

    #[test]
    fn legacy_files_stay_version_one_and_native_metadata_roundtrips_in_version_two() {
        let mut data = MsegData::default();
        let legacy = CurveShape::capture(&data).to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&legacy).unwrap();
        assert_eq!(json["version"], 1);
        assert_eq!(json["nodes"][0].as_object().unwrap().len(), 4);
        let old_project = serde_json::to_string(&data).unwrap();
        assert!(!old_project.contains("shape") && !old_project.contains("weight"));
        assert_eq!(
            serde_json::from_str::<MsegData>(&old_project).unwrap(),
            data
        );
        data.nodes[0].shape = SegmentShape::SCurve;
        data.nodes[0].weight = PointWeight::Medium;
        data.nodes[1].weight = PointWeight::Soft;
        let shape = CurveShape::capture(&data);
        let native = shape.to_json().unwrap();
        let json: serde_json::Value = serde_json::from_str(&native).unwrap();
        assert_eq!(json["version"], 2);
        assert_eq!(CurveShape::from_json(&native).unwrap(), shape);
        assert!(
            CurveShape::from_json(&native.replace("\"version\": 2", "\"version\": 1")).is_err()
        );
        assert!(
            CurveShape::from_json(&native.replace("\"version\": 2", "\"version\": 3")).is_err()
        );
        let project = serde_json::to_string(&data).unwrap();
        assert_eq!(serde_json::from_str::<MsegData>(&project).unwrap(), data);
    }

    #[test]
    fn s_curve_pen_uses_one_native_segment() {
        let nodes = pen_nodes(MsegTool::SCurve, (0.2, 0.1), (0.8, 0.9));
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].shape, SegmentShape::SCurve);
        assert_eq!((nodes[0].time, nodes[1].time), (0.2, 0.8));
    }

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
