//! Six Pack: 6-band parallel multiband saturator.

use nih_plug::prelude::*;
use std::sync::Arc;

pub mod bands;
pub mod oversampling;
pub mod saturation;
pub mod spectrum;
pub mod svf;

pub struct SixPack {
    params: Arc<SixPackParams>,
}

#[derive(Default, Params)]
pub struct SixPackParams {}

impl Default for SixPack {
    fn default() -> Self {
        Self {
            params: Arc::new(SixPackParams::default()),
        }
    }
}

impl Plugin for SixPack {
    const NAME: &'static str = "Six Pack";
    const VENDOR: &'static str = "mpd";
    const URL: &'static str = "";
    const EMAIL: &'static str = "";
    const VERSION: &'static str = "0.1.0";
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

    fn process(
        &mut self,
        _buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        _ctx: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        ProcessStatus::Normal
    }
}

impl ClapPlugin for SixPack {
    const CLAP_ID: &'static str = "com.mpd.six-pack";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("6-band parallel multiband saturator");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Distortion,
    ];
}

impl Vst3Plugin for SixPack {
    const VST3_CLASS_ID: [u8; 16] = *b"mpdSixPack0001AB";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(SixPack);
nih_export_vst3!(SixPack);
