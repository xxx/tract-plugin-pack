//! Background-to-audio processor transfer with deferred destruction.

use crate::engine::{PrepareKey, Processor};
use std::sync::Mutex;

#[derive(Default)]
struct Slot {
    processor: Option<Box<Processor>>,
    ready: Option<PrepareKey>,
}

#[derive(Default)]
pub struct ProcessorHandoff {
    slot: Mutex<Slot>,
}

impl ProcessorHandoff {
    /// Background thread only: replacing this slot also reclaims a retired engine.
    pub fn publish(&self, processor: Box<Processor>) {
        if let Ok(mut slot) = self.slot.lock() {
            slot.ready = Some(processor.key);
            slot.processor = Some(processor);
        }
    }

    pub fn is_ready(&self, key: PrepareKey) -> bool {
        self.slot.lock().is_ok_and(|slot| slot.ready == Some(key))
    }

    /// Audio thread: swap ownership without allocating or destroying either engine.
    pub fn try_install(&self, key: PrepareKey, active: &mut Box<Processor>) -> bool {
        let Ok(mut slot) = self.slot.try_lock() else {
            return false;
        };
        if slot.ready != Some(key) {
            return false;
        }
        let Some(prepared) = slot.processor.as_mut() else {
            return false;
        };
        std::mem::swap(active, prepared);
        slot.ready = None;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::neutral_modulation;
    use tract_effects::effects::{EffectKind, TrackEffect, default_params_for_kind};

    fn prepared(kind: EffectKind, generation: u64) -> Box<Processor> {
        let track = TrackEffect {
            kind,
            params: default_params_for_kind(kind),
            ..TrackEffect::default()
        };
        Box::new(Processor::new(
            PrepareKey {
                kind,
                generation,
                sample_rate_bits: 48000.0_f32.to_bits(),
            },
            &track,
            neutral_modulation(),
        ))
    }

    #[test]
    fn adoption_retires_old_engine_without_destruction_or_swap_back() {
        let handoff = ProcessorHandoff::default();
        let mut active = prepared(EffectKind::Svf, 1);
        let old = active.key;
        let new = prepared(EffectKind::Delay, 2);
        let wanted = new.key;
        handoff.publish(new);
        assert_no_alloc::assert_no_alloc(|| assert!(handoff.try_install(wanted, &mut active)));
        assert_eq!(active.key, wanted);
        assert_eq!(
            handoff.slot.lock().unwrap().processor.as_ref().unwrap().key,
            old
        );
        assert!(!handoff.try_install(wanted, &mut active));
    }

    #[test]
    fn stale_selection_and_sample_rate_never_replace_the_current_engine() {
        let handoff = ProcessorHandoff::default();
        let mut active = prepared(EffectKind::None, 1);
        let pending = prepared(EffectKind::Svf, 2);
        let key = pending.key;
        handoff.publish(pending);
        assert!(!handoff.try_install(
            PrepareKey {
                generation: 3,
                ..key
            },
            &mut active
        ));
        assert!(!handoff.try_install(
            PrepareKey {
                sample_rate_bits: 96000.0_f32.to_bits(),
                ..key
            },
            &mut active
        ));
        assert_eq!(active.key.kind, EffectKind::None);
        assert!(handoff.try_install(key, &mut active));
    }

    #[test]
    fn contended_mailbox_is_a_nonblocking_miss() {
        let handoff = ProcessorHandoff::default();
        let mut active = prepared(EffectKind::None, 1);
        let key = active.key;
        let _guard = handoff.slot.lock().unwrap();
        assert!(!handoff.try_install(key, &mut active));
    }
}
