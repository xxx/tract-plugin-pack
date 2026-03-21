fn main() {
    // Use mold linker on Linux if available, for faster link times.
    // Falls back to the default linker silently if mold is not installed.
    if cfg!(target_os = "linux") {
        let mold_available = std::process::Command::new("mold")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        if mold_available {
            println!("cargo:rustc-link-arg=-fuse-ld=mold");
        }
    }
}
