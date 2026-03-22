use anyhow::Result;
use std::env;
use std::process::Command;

fn main() -> Result<()> {
    let task = env::args().nth(1);
    match task.as_deref() {
        Some("bundle") => bundle()?,
        Some("bundle-universal") => bundle_universal()?,
        _ => print_help(),
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Tasks:
  bundle            Build plugin bundles
  bundle-universal  Build universal (multi-architecture) plugin bundles"
    );
}

fn bundle() -> Result<()> {
    println!("Building plugin bundles...");

    let status = Command::new("cargo")
        .args(["xtask", "bundle", "wavetable-filter", "--release"])
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed");
    }

    println!("Build complete!");
    Ok(())
}

fn bundle_universal() -> Result<()> {
    println!("Building universal plugin bundles...");

    let status = Command::new("cargo")
        .args(["xtask", "bundle-universal", "wavetable-filter", "--release"])
        .status()?;

    if !status.success() {
        anyhow::bail!("Build failed");
    }

    println!("Build complete!");
    Ok(())
}
