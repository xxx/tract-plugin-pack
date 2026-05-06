//! Imagine: multiband stereo imager modeled on Ozone Imager.

#![feature(portable_simd)]

use nih_plug::prelude::*;
use std::sync::Arc;

pub mod bands;
pub mod crossover;
pub mod decorrelator;
pub mod hilbert;
pub mod midside;
pub mod spectrum;
pub mod vectorscope;

pub struct Imagine {
    params: Arc<ImagineParams>,
}

#[derive(Params, Default)]
pub struct ImagineParams {}

impl Default for Imagine {
    fn default() -> Self {
        Self {
            params: Arc::new(ImagineParams::default()),
        }
    }
}

impl Plugin for Imagine {
    const NAME: &'static str = "Imagine";
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

impl ClapPlugin for Imagine {
    const CLAP_ID: &'static str = "com.mpd.imagine";
    const CLAP_DESCRIPTION: Option<&'static str> = Some("Multiband stereo imager");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Utility,
    ];
}

impl Vst3Plugin for Imagine {
    const VST3_CLASS_ID: [u8; 16] = *b"mpdImagine0001AB";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Stereo];
}

nih_export_clap!(Imagine);
nih_export_vst3!(Imagine);
