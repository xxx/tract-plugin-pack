use super::shape::validate_nodes;
use super::{CurveShape, ShapeError};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub const WAVE_PALETTE_SLOTS: usize = 8;
const PALETTE_VERSION: u32 = 1;

/// Reset project waves when loading a preset saved before the bank existed.
pub fn prepare_wave_palette_state(fields: &mut std::collections::BTreeMap<String, String>) {
    fields.entry("wave-palette".to_owned()).or_insert_with(|| {
        format!(
            r#"{{"version":{PALETTE_VERSION},"slots":[{}]}}"#,
            ["null"; WAVE_PALETTE_SLOTS].join(",")
        )
    });
}

/// GUI/preset wave storage, separate from the fixed-capacity audio document.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WavePalette {
    slots: [Option<CurveShape>; WAVE_PALETTE_SLOTS],
}

impl WavePalette {
    pub fn get(&self, slot: usize) -> Option<&CurveShape> {
        self.slots.get(slot).and_then(Option::as_ref)
    }

    pub fn store(&mut self, slot: usize, shape: &CurveShape) -> Result<bool, ShapeError> {
        let destination = self.slots.get_mut(slot).ok_or(ShapeError::Invalid)?;
        validate_nodes(shape.nodes())?;
        if destination.as_ref() == Some(shape) {
            return Ok(false);
        }
        *destination = Some(shape.clone());
        Ok(true)
    }

    pub fn clear(&mut self, slot: usize) -> Result<bool, ShapeError> {
        Ok(self
            .slots
            .get_mut(slot)
            .ok_or(ShapeError::Invalid)?
            .take()
            .is_some())
    }
}

impl Serialize for WavePalette {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        struct Saved<'a> {
            version: u32,
            slots: &'a [Option<CurveShape>; WAVE_PALETTE_SLOTS],
        }
        Saved {
            version: PALETTE_VERSION,
            slots: &self.slots,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for WavePalette {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Saved {
            version: u32,
            slots: [Option<CurveShape>; WAVE_PALETTE_SLOTS],
        }
        let saved = Saved::deserialize(deserializer)?;
        if saved.version != PALETTE_VERSION {
            return Err(serde::de::Error::custom("Unsupported wave palette version"));
        }
        Ok(Self { slots: saved.slots })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{
        CurveShape, FactoryShape, MAX_NODES, MsegData, PointWeight, SegmentShape, ShapeError,
        factory_shape,
    };

    #[test]
    fn missing_preset_bank_gets_an_empty_record_without_overwriting_supplied_slots() {
        let mut fields = std::collections::BTreeMap::new();
        fields.insert("other-state".to_owned(), "keep".to_owned());
        prepare_wave_palette_state(&mut fields);
        assert_eq!(
            serde_json::from_str::<WavePalette>(&fields["wave-palette"]).unwrap(),
            WavePalette::default()
        );
        let mut palette = WavePalette::default();
        palette
            .store(0, &factory_shape(FactoryShape::Duck))
            .unwrap();
        fields.insert(
            "wave-palette".to_owned(),
            serde_json::to_string(&palette).unwrap(),
        );
        let before = fields.clone();
        prepare_wave_palette_state(&mut fields);
        assert_eq!(fields, before);
        assert_eq!(fields["other-state"], "keep");
    }

    #[test]
    fn slots_are_independent_validated_and_store_clear_are_idempotent() {
        let mut palette = WavePalette::default();
        assert!((0..WAVE_PALETTE_SLOTS).all(|slot| palette.get(slot).is_none()));
        let shape = factory_shape(FactoryShape::Duck);
        assert_eq!(palette.store(3, &shape), Ok(true));
        assert_eq!(palette.store(3, &shape), Ok(false));
        assert_eq!(palette.get(3), Some(&shape));
        assert!(palette.get(2).is_none());
        let before = palette.clone();
        assert_eq!(
            palette.store(WAVE_PALETTE_SLOTS, &shape),
            Err(ShapeError::Invalid)
        );
        assert_eq!(palette.clear(WAVE_PALETTE_SLOTS), Err(ShapeError::Invalid));
        let invalid = CurveShape {
            nodes: vec![shape.nodes()[0]; MAX_NODES + 1],
        };
        assert_eq!(palette.store(3, &invalid), Err(ShapeError::Capacity));
        assert_eq!(palette, before);
        assert_eq!(palette.clear(3), Ok(true));
        assert_eq!(palette.clear(3), Ok(false));
        assert_eq!(palette, WavePalette::default());
    }

    #[test]
    fn palette_round_trips_native_and_legacy_shape_records_in_all_slots() {
        let mut palette = WavePalette::default();
        let mut data = MsegData::default();
        data.nodes[0].shape = SegmentShape::SCurve;
        data.nodes[0].tension = -0.4;
        data.nodes[1].weight = PointWeight::Soft;
        for slot in 0..WAVE_PALETTE_SLOTS {
            let shape = if slot % 2 == 0 {
                CurveShape::capture(&data)
            } else {
                factory_shape(FactoryShape::Square)
            };
            palette.store(slot, &shape).unwrap();
        }
        let json = serde_json::to_string(&palette).unwrap();
        let restored: WavePalette = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, palette);
        let mut curve = MsegData {
            beats: 7.5,
            time_divisions: 12,
            ..MsegData::default()
        };
        restored.get(0).unwrap().apply(&mut curve, false).unwrap();
        assert_eq!(curve.active(), data.active());
        assert_eq!((curve.beats, curve.time_divisions), (7.5, 12));
        assert_eq!(
            serde_json::from_str::<WavePalette>(
                &serde_json::to_string(&WavePalette::default()).unwrap()
            )
            .unwrap(),
            WavePalette::default()
        );
    }

    #[test]
    fn invalid_palette_versions_lengths_and_geometry_are_rejected() {
        let mut palette = WavePalette::default();
        palette
            .store(0, &factory_shape(FactoryShape::Triangle))
            .unwrap();
        let original = serde_json::to_value(&palette).unwrap();
        let mut invalid = original.clone();
        invalid["version"] = 2.into();
        assert!(serde_json::from_value::<WavePalette>(invalid).is_err());
        for length in [0, WAVE_PALETTE_SLOTS - 1, WAVE_PALETTE_SLOTS + 1] {
            let mut invalid = original.clone();
            invalid["slots"] = serde_json::Value::Array(vec![serde_json::Value::Null; length]);
            assert!(serde_json::from_value::<WavePalette>(invalid).is_err());
        }
        let mut invalid = original.clone();
        invalid["slots"][0]["nodes"][1]["time"] = 0.into();
        assert!(serde_json::from_value::<WavePalette>(invalid).is_err());
        let mut invalid = original.clone();
        invalid["slots"][0]["version"] = 99.into();
        assert!(serde_json::from_value::<WavePalette>(invalid).is_err());
        let mut invalid = original;
        invalid["slots"][0]["nodes"][1]["value"] = 2.into();
        assert!(serde_json::from_value::<WavePalette>(invalid).is_err());
    }
}
