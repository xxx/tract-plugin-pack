fn main() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .unwrap_or_else(|| "gated-preview.png".to_owned());
    let width = args.next().and_then(|arg| arg.parse().ok()).unwrap_or(480);
    let height = args.next().and_then(|arg| arg.parse().ok()).unwrap_or(840);
    gated::render_preview(std::path::Path::new(&path), width, height)
}
