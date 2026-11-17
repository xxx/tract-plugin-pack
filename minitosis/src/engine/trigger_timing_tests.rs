use super::*;
use tiny_skia_widgets::{MsegData, PlayMode, SyncMode};
use tract_effects::effects::default_params_for_kind;
use tract_effects::modulation::{
    TransientAlgorithm, TransientSettings, TriggerSource, TriggerTiming,
};

fn setup() -> (Processor, TrackEffect) {
    let track = TrackEffect {
        kind: EffectKind::Bitcrush,
        params: default_params_for_kind(EffectKind::Bitcrush),
        ..TrackEffect::default()
    };
    let cfg = TrackModulation {
        trigger: TriggerSource::TransientSample {
            threshold: 1.5,
            hold_ms: 5.0,
        },
        detector_response: TransientSettings {
            algorithm: TransientAlgorithm::Peak,
            detail: 100.0,
        },
        trigger_timing: TriggerTiming {
            audio_delay_ms: 3.0,
            blend_ms: 1.0,
        },
        msegs: std::array::from_fn(|_| MsegData {
            play_mode: PlayMode::Cyclic,
            sync_mode: SyncMode::Time,
            time_seconds: 1.0,
            ..MsegData::default()
        }),
        targets: [None; 3],
        ..TrackModulation::default()
    };
    (
        Processor::new(
            PrepareKey {
                kind: track.kind,
                generation: 1,
                sample_rate_bits: 48000.0_f32.to_bits(),
            },
            &track,
            cfg,
        ),
        track,
    )
}
#[test]
fn queued_audio_resets_land_at_exact_samples_and_audio_is_host_partition_invariant() {
    let (mut actual, track) = setup();
    let (mut reference, _) = setup();
    let input: [f32; 1024] =
        std::array::from_fn(|i| if [0, 256, 700].contains(&i) { 0.8 } else { 0.0 });
    let (mut output, mut expected) = ([[0.0; 2]; 1024], [[0.0; 2]; 1024]);
    let mut times = [usize::MAX; 16];
    let mut count = 0;
    assert_no_alloc::assert_no_alloc(|| {
        reference.begin_block(&track, &input, &input, 120.0);
        for (i, value) in input.into_iter().enumerate() {
            expected[i] = reference.process_sample([value; 2], 1.0, false);
        }
        let mut start = 0;
        let mut epoch = 0;
        for n in [1, 7, 136, 1, 111, 144, 1, 299, 144, 1, 179] {
            actual.begin_block(
                &track,
                &input[start..start + n],
                &input[start..start + n],
                120.0,
            );
            for i in start..start + n {
                output[i] = actual.process_sample([input[i]; 2], 1.0, false);
                let after = actual.state.trigger_epoch();
                if after != epoch {
                    times[count] = i;
                    count += 1;
                }
                epoch = after;
            }
            start += n;
        }
    });
    assert_eq!(count, 3);
    assert_eq!(&times[..3], &[144, 400, 844]);
    assert_eq!(output, expected);
    assert_eq!(actual.modulation_frame(), reference.modulation_frame());
    assert_eq!(actual.latency(), 0);
}
