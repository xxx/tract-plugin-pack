use nih_plug::prelude::*;
use std::sync::Arc;

pub mod editor;
pub mod spectral;

// ── Plugin struct ──────────────────────────────────────────────────────────────

pub struct Satch {
    params: Arc<SatchParams>,
    spectral_l: spectral::SpectralClipper,
    spectral_r: spectral::SpectralClipper,
    // Dry signal delay to align with spectral latency
    dry_delay_l: Vec<f32>,
    dry_delay_r: Vec<f32>,
    dry_delay_pos: usize,
}

// ── Params ─────────────────────────────────────────────────────────────────────

#[derive(Params)]
pub struct SatchParams {
    #[persist = "editor-state"]
    pub editor_state: Arc<editor::SatchEditorState>,

    /// Drive: stored as linear gain, displayed/edited in dB.
    #[id = "drive"]
    pub drive: FloatParam,

    /// Detail: 0–100%, controls spectral detail preservation.
    #[id = "detail"]
    pub detail: FloatParam,

    /// Mix: 0–100%, dry/wet blend.
    #[id = "mix"]
    pub mix: FloatParam,
}

impl Default for Satch {
    fn default() -> Self {
        Self {
            params: Arc::new(SatchParams::new()),
            spectral_l: spectral::SpectralClipper::new(2048, 512),
            spectral_r: spectral::SpectralClipper::new(2048, 512),
            dry_delay_l: vec![0.0; 2048],
            dry_delay_r: vec![0.0; 2048],
            dry_delay_pos: 0,
        }
    }
}

impl SatchParams {
    fn new() -> Self {
        Self {
            editor_state: editor::SatchEditorState::default_state(),
            drive: FloatParam::new(
                "Drive",
                util::db_to_gain(0.0),
                FloatRange::Skewed {
                    min: util::db_to_gain(0.0),
                    max: util::db_to_gain(24.0),
                    factor: FloatRange::gain_skew_factor(0.0, 24.0),
                },
            )
            .with_smoother(SmoothingStyle::Linear(50.0))
            .with_unit(" dB")
            .with_value_to_string(formatters::v2s_f32_gain_to_db(1))
            .with_string_to_value(formatters::s2v_f32_gain_to_db()),

            detail: FloatParam::new(
                "Detail",
                0.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),

            mix: FloatParam::new(
                "Mix",
                100.0,
                FloatRange::Linear {
                    min: 0.0,
                    max: 100.0,
                },
            )
            .with_unit(" %")
            .with_value_to_string(formatters::v2s_f32_rounded(0)),
        }
    }
}

// ── Plugin impl ────────────────────────────────────────────────────────────────

impl Plugin for Satch {
    const NAME: &'static str = "satch";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    const SAMPLE_ACCURATE_AUTOMATION: bool = true;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone())
    }

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        _buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.spectral_l.reset();
        self.spectral_r.reset();
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
        context.set_latency_samples(self.spectral_l.latency_samples() as u32);
        true
    }

    fn reset(&mut self) {
        self.spectral_l.reset();
        self.spectral_r.reset();
        self.dry_delay_l.fill(0.0);
        self.dry_delay_r.fill(0.0);
        self.dry_delay_pos = 0;
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let num_samples = buffer.samples();
        if num_samples == 0 {
            return ProcessStatus::Normal;
        }

        let channel_slices = buffer.as_slice();
        if channel_slices.len() < 2 {
            return ProcessStatus::Normal;
        }
        let (first, rest) = channel_slices.split_at_mut(1);
        let left = &mut first[0][..num_samples];
        let right = &mut rest[0][..num_samples];

        let detail = self.params.detail.value() / 100.0;
        let mix = self.params.mix.value() / 100.0;

        // Skip FFT processing when the spectral result won't be used.
        // detail=0: spectral contribution is multiplied by 0 in the blend.
        // mix=0: entire wet path is unused (output = dry).
        let skip_fft = detail == 0.0 || mix == 0.0;

        for i in 0..num_samples {
            let drive = self.params.drive.smoothed.next();
            let (amount, drive_linear) = spectral::compute_drive_params(drive);

            let in_l = left[i];
            let in_r = right[i];

            // Delay dry signal to align with spectral path latency
            let dry_l = self.dry_delay_l[self.dry_delay_pos];
            let dry_r = self.dry_delay_r[self.dry_delay_pos];
            self.dry_delay_l[self.dry_delay_pos] = in_l;
            self.dry_delay_r[self.dry_delay_pos] = in_r;
            self.dry_delay_pos = (self.dry_delay_pos + 1) % self.dry_delay_l.len();

            // Time-domain saturation path (on delayed dry signal).
            // Returns both the saturated output and tanh(drive*x) for reuse
            // in the clip-aware blend (avoids computing tanh twice per channel).
            let (td_l, tanh_l) = spectral::saturate_td_with_tanh(dry_l, amount, drive_linear);
            let (td_r, tanh_r) = spectral::saturate_td_with_tanh(dry_r, amount, drive_linear);

            // Spectral path (has built-in latency from STFT).
            // When skip_fft=true, ring buffer state is maintained but FFT
            // frames are not computed (saves ~95% of CPU in this path).
            let sp_l = if skip_fft {
                self.spectral_l.process_sample_skip_fft(in_l, amount, drive_linear)
            } else {
                self.spectral_l.process_sample(in_l, amount, drive_linear)
            };
            let sp_r = if skip_fft {
                self.spectral_r.process_sample_skip_fft(in_r, amount, drive_linear)
            } else {
                self.spectral_r.process_sample(in_r, amount, drive_linear)
            };

            // Clip-aware detail blend: only restore spectral detail where clipping occurs.
            // tanh²(drive * x) ≈ 0 in the linear region, ≈ 1 when fully saturated.
            // Reuse tanh from saturate_td (same argument: drive * dry).
            let clip_l = tanh_l * tanh_l;
            let clip_r = tanh_r * tanh_r;
            let lost_l = sp_l - td_l;
            let lost_r = sp_r - td_r;
            let wet_l = td_l + detail * clip_l * lost_l;
            let wet_r = td_r + detail * clip_r * lost_r;

            // Dry/wet mix (dry is delayed to align)
            left[i] = mix * wet_l + (1.0 - mix) * dry_l;
            right[i] = mix * wet_r + (1.0 - mix) * dry_r;
        }

        ProcessStatus::Normal
    }
}

// ── CLAP / VST3 ────────────────────────────────────────────────────────────────

impl ClapPlugin for Satch {
    const CLAP_ID: &'static str = "com.mpd.satch";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("A detail-preserving spectral saturator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Distortion,
    ];
}

impl Vst3Plugin for Satch {
    const VST3_CLASS_ID: [u8; 16] = *b"SatchMpdPlugin\0\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] = &[
        Vst3SubCategory::Fx,
        Vst3SubCategory::Distortion,
    ];
}

nih_export_clap!(Satch);
nih_export_vst3!(Satch);
