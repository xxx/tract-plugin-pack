use anyhow::Result;
use std::env;
use std::process::Command;

fn main() -> Result<()> {
    let mut args = env::args().skip(1);
    let task = args.next();
    let rest: Vec<String> = args.collect();
    match task.as_deref() {
        Some("bundle") => bundle()?,
        Some("bundle-universal") => bundle_universal()?,
        Some("native") => run_native(&rest)?,
        _ => print_help(),
    }
    Ok(())
}

fn print_help() {
    eprintln!(
        "Tasks:
  bundle                      Build plugin bundles
  bundle-universal            Build universal (multi-architecture) plugin bundles
  native <cargo args...>      Run `cargo <args>` with target-cpu auto-tuned to the build host.
                              On x86_64 with AVX2+FMA+BMI2 this sets -C target-cpu=haswell,
                              which lets tiny-skia and auto-vectorized code use AVX2 paths.

Examples:
  cargo xtask native nih-plug bundle wavetable-filter --release
  cargo xtask native build --release --bin gs-meter"
    );
}

fn detect_target_cpu() -> Option<&'static str> {
    #[cfg(target_arch = "x86_64")]
    {
        let avx2 = std::arch::is_x86_feature_detected!("avx2");
        let fma = std::arch::is_x86_feature_detected!("fma");
        let bmi2 = std::arch::is_x86_feature_detected!("bmi2");
        if avx2 && fma && bmi2 {
            return Some("haswell");
        }
    }
    None
}

fn run_native(args: &[String]) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("usage: cargo xtask native <cargo args...>");
    }

    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();

    match detect_target_cpu() {
        Some(cpu) if !rustflags.contains("target-cpu=") => {
            if !rustflags.is_empty() {
                rustflags.push(' ');
            }
            rustflags.push_str("-C target-cpu=");
            rustflags.push_str(cpu);
            eprintln!(
                "[xtask] build host supports AVX2+FMA+BMI2 -- using -C target-cpu={}",
                cpu
            );
        }
        Some(_) => {
            eprintln!("[xtask] RUSTFLAGS already sets target-cpu; leaving it alone");
        }
        None => {
            eprintln!(
                "[xtask] no AVX2/FMA/BMI2 detected on build host; using default target-cpu"
            );
        }
    }

    let status = Command::new("cargo")
        .args(args)
        .env("RUSTFLAGS", &rustflags)
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo {} failed", args.join(" "));
    }
    Ok(())
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
