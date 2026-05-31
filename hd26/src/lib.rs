use nih_plug::prelude::*;
use std::sync::Arc;

// DSP modules (uncommented as they are added in later tasks):
pub mod lfo;
pub mod delay;
pub mod transient;
// pub mod hyper;
// pub mod dimension;
// mod editor;

pub struct Hd26 {
    params: Arc<Hd26Params>,
    sample_rate: f32,
}

#[derive(Params)]
pub struct Hd26Params {}

impl Default for Hd26 {
    fn default() -> Self {
        Self {
            params: Arc::new(Hd26Params::new()),
            sample_rate: 48_000.0,
        }
    }
}

impl Hd26Params {
    fn new() -> Self {
        Self {}
    }
}

impl Plugin for Hd26 {
    const NAME: &'static str = "HD26";
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

    fn initialize(
        &mut self,
        _layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        true
    }

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for Hd26 {
    const CLAP_ID: &'static str = "com.mpd.hd26";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Serum-style Hyper/Dimension chorus + widener");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Chorus,
        ClapFeature::Stereo,
    ];
}

impl Vst3Plugin for Hd26 {
    const VST3_CLASS_ID: [u8; 16] = *b"HD26HyperDimMpd\0";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Modulation];
}

nih_export_clap!(Hd26);
nih_export_vst3!(Hd26);
