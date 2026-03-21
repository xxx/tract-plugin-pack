use wavetable_filter::WavetableFilter;

fn main() {
    nih_plug::wrapper::standalone::nih_export_standalone::<WavetableFilter>();
}
