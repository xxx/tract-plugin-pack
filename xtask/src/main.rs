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
        Some("bench-compare") => bench_compare()?,
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
  bench-compare               Run bench-suite twice (target-cpu=x86-64 vs haswell) and
                              print criterion's delta report.

Examples:
  cargo xtask native nih-plug bundle wavetable-filter --release
  cargo xtask native build --release --bin gs-meter
  cargo xtask bench-compare"
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
            eprintln!("[xtask] no AVX2/FMA/BMI2 detected on build host; using default target-cpu");
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

fn bench_compare() -> Result<()> {
    // Each run rebuilds bench-suite with a different target-cpu. We pin the linker to
    // mold explicitly because passing RUSTFLAGS via env fully overrides rustflags from
    // ~/.cargo/config.toml, so we'd otherwise lose the mold setting for the duration
    // of the run.
    let linker = "-C link-arg=-fuse-ld=mold";

    // (package, bench target name). Serial execution keeps the CPU from contending
    // with itself and skips each package's lib/main libtest harnesses (which reject
    // criterion's --save-baseline flag).
    let bench_targets: &[(&str, &str)] = &[
        ("bench-suite", "render"),
        ("pope-scope", "dsp"),
        ("wavetable-filter", "dsp"),
        ("warp-zone", "dsp"),
    ];

    let run = |label: &str, cpu: &str, criterion_args: &[&str]| -> Result<()> {
        let flags = format!("{linker} -C target-cpu={cpu}");
        eprintln!("\n=== bench run: {label} (target-cpu={cpu}) ===");
        for (pkg, bench) in bench_targets {
            eprintln!("--- {pkg}::{bench} ---");
            let mut args: Vec<&str> = vec!["bench", "-p", pkg, "--bench", bench, "--"];
            args.extend_from_slice(criterion_args);
            let status = Command::new("cargo")
                .args(&args)
                .env("RUSTFLAGS", &flags)
                .status()?;
            if !status.success() {
                anyhow::bail!("bench run '{label}' ({pkg}::{bench}) failed");
            }
        }
        Ok(())
    };

    run("baseline", "x86-64", &["--save-baseline", "baseline"])?;
    run("haswell", "haswell", &["--save-baseline", "haswell"])?;

    eprintln!("\n=== comparison: haswell vs baseline ===");
    // Rerun with RUSTFLAGS matching the haswell build so the binary doesn't get
    // rebuilt, and point criterion at the saved baseline for % deltas.
    run("compare", "haswell", &["--baseline", "baseline"])?;
    Ok(())
}

fn bundle() -> Result<()> {
    println!("Building plugin bundles...");

    for plugin in &["wavetable-filter", "miff"] {
        let status = Command::new("cargo")
            .args(["xtask", "bundle", plugin, "--release"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Build failed for {}", plugin);
        }
    }

    println!("Build complete!");
    Ok(())
}

fn bundle_universal() -> Result<()> {
    println!("Building universal plugin bundles...");

    for plugin in &["wavetable-filter", "miff"] {
        let status = Command::new("cargo")
            .args(["xtask", "bundle-universal", plugin, "--release"])
            .status()?;
        if !status.success() {
            anyhow::bail!("Build failed for {}", plugin);
        }
    }

    println!("Build complete!");
    Ok(())
}
