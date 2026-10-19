use std::{env, path::Path, process::Command};

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BuildMode {
    Redistributable,
    Native,
}

#[derive(Debug)]
struct Options {
    mode: BuildMode,
    packages: Vec<String>,
    cargo_args: Vec<String>,
}

impl Options {
    fn parse(args: &[String]) -> Result<Self> {
        let mut mode = None;
        let mut packages = Vec::new();
        let mut cargo_args = Vec::new();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            if arg == "-p" || arg == "--package" || arg.starts_with("--package=") {
                let name = match arg.strip_prefix("--package=") {
                    Some(name) => name,
                    None => args
                        .next()
                        .context("Missing package name after -p/--package")?,
                };
                ensure!(
                    !name.is_empty() && !name.starts_with('-'),
                    "Missing package name after {arg}"
                );
                packages.push(name.to_owned());
                continue;
            }
            let selected = match arg.as_str() {
                "--native" => Some(BuildMode::Native),
                "--redistributable" => Some(BuildMode::Redistributable),
                _ => None,
            };
            if let Some(selected) = selected {
                ensure!(
                    mode.is_none_or(|mode| mode == selected),
                    "--native and --redistributable cannot be combined"
                );
                mode = Some(selected);
            } else {
                cargo_args.push(if arg == "-r" {
                    "--release".into()
                } else {
                    arg.clone()
                });
            }
        }
        if !cargo_args.iter().any(|arg| {
            matches!(arg.as_str(), "--release" | "--profile") || arg.starts_with("--profile=")
        }) {
            cargo_args.push("--release".into());
        }
        Ok(Self {
            mode: mode.unwrap_or(BuildMode::Redistributable),
            packages,
            cargo_args,
        })
    }
}

#[derive(Deserialize)]
struct Metadata {
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    name: String,
    targets: Vec<Target>,
}

#[derive(Deserialize)]
struct Target {
    crate_types: Vec<String>,
}

fn plugin_names(metadata: &[u8]) -> Result<Vec<String>> {
    let metadata: Metadata = serde_json::from_slice(metadata).context("Invalid cargo metadata")?;
    let mut names: Vec<_> = metadata
        .packages
        .into_iter()
        .filter(|package| {
            package
                .targets
                .iter()
                .any(|target| target.crate_types.iter().any(|kind| kind == "cdylib"))
        })
        .map(|package| package.name)
        .collect();
    names.sort();
    ensure!(!names.is_empty(), "No plugin crates found in the workspace");
    Ok(names)
}

fn cpu_flags(flags: &[String], cpu: &str) -> Vec<String> {
    let mut result = Vec::new();
    let mut args = flags.iter().peekable();
    while let Some(arg) = args.next() {
        let separate_value = arg == "-C" || arg == "--codegen";
        let option = if separate_value {
            args.peek().map(|arg| arg.as_str()).unwrap_or_default()
        } else {
            arg.strip_prefix("-C")
                .or_else(|| arg.strip_prefix("--codegen="))
                .unwrap_or_default()
        };
        if option.starts_with("target-cpu=") || option.starts_with("target-feature=") {
            if separate_value {
                args.next();
            }
        } else {
            result.push(arg.clone());
        }
    }
    result.extend(["-C".into(), format!("target-cpu={cpu}")]);
    result
}

pub(super) fn run(task: &str, args: &[String]) -> Result<()> {
    let options = Options::parse(args)?;
    ensure!(
        task != "bundle-universal" || options.mode != BuildMode::Native,
        "--native cannot be used with bundle-universal; use redistributable builds for both architectures"
    );
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let plugins = if options.packages.is_empty() {
        let metadata = Command::new("cargo")
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .current_dir(&root)
            .output()
            .context("Could not run cargo metadata")?;
        ensure!(
            metadata.status.success(),
            "cargo metadata failed: {}",
            String::from_utf8_lossy(&metadata.stderr)
        );
        plugin_names(&metadata.stdout)?
    } else {
        options.packages
    };
    let cpu = match options.mode {
        BuildMode::Redistributable => "generic",
        BuildMode::Native => super::detect_target_cpu().unwrap_or("generic"),
    };
    let inherited: Vec<String> = match env::var("CARGO_ENCODED_RUSTFLAGS") {
        Ok(flags) => flags
            .split('\x1f')
            .filter(|arg| !arg.is_empty())
            .map(String::from)
            .collect(),
        Err(_) => env::var("RUSTFLAGS")
            .unwrap_or_default()
            .split_whitespace()
            .map(String::from)
            .collect(),
    };
    // Set build flags on a child process without mutating this process's environment.
    let mut command =
        Command::new(env::current_exe().context("Could not locate xtask executable")?);
    command.args(["__bundle", task]);
    for plugin in &plugins {
        command.args(["-p", plugin]);
    }
    command
        .args(&options.cargo_args)
        .current_dir(root)
        // Override config CPU flags as well as inherited environment flags.
        .env(
            "CARGO_ENCODED_RUSTFLAGS",
            cpu_flags(&inherited, cpu).join("\x1f"),
        );
    eprintln!(
        "[xtask] Building {} plugins ({:?}, target-cpu={cpu}): {}",
        plugins.len(),
        options.mode,
        plugins.join(", ")
    );
    let status = command
        .status()
        .context("Could not run the integrated bundler")?;
    ensure!(status.success(), "cargo xtask {task} failed");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| String::from(*arg)).collect()
    }

    #[test]
    fn defaults_to_redistributable_release() {
        let options = Options::parse(&[]).unwrap();
        assert_eq!(options.mode, BuildMode::Redistributable);
        assert_eq!(options.cargo_args, ["--release"]);
    }

    #[test]
    fn native_mode_forwards_build_options() {
        let options = Options::parse(&strings(&[
            "--native",
            "--locked",
            "--target",
            "x86_64-unknown-linux-gnu",
        ]))
        .unwrap();
        assert_eq!(options.mode, BuildMode::Native);
        assert_eq!(
            options.cargo_args,
            [
                "--locked",
                "--target",
                "x86_64-unknown-linux-gnu",
                "--release"
            ]
        );
    }

    #[test]
    fn explicit_packages_are_separated_from_build_options() {
        let options = Options::parse(&strings(&[
            "--native",
            "--locked",
            "-p",
            "gs-meter",
            "--package",
            "gain-brain",
            "--profile=profiling",
        ]))
        .unwrap();
        assert_eq!(options.packages, ["gs-meter", "gain-brain"]);
        assert_eq!(options.cargo_args, ["--locked", "--profile=profiling"]);
    }

    #[test]
    fn accepts_package_with_equals() {
        let options = Options::parse(&strings(&["--package=gs-meter"])).unwrap();
        assert_eq!(options.packages, ["gs-meter"]);
        assert_eq!(options.cargo_args, ["--release"]);
    }

    #[test]
    fn rejects_missing_package_names() {
        for args in [&["-p"][..], &["--package", "--locked"], &["--package="]] {
            assert!(Options::parse(&strings(args)).is_err(), "{args:?}");
        }
    }

    #[test]
    fn universal_rejects_native_mode_before_building() {
        assert!(run("bundle-universal", &strings(&["--native"])).is_err());
    }

    #[test]
    fn rejects_conflicting_modes() {
        assert!(Options::parse(&strings(&["--native", "--redistributable"])).is_err());
    }

    #[test]
    fn preserves_explicit_profiles() {
        for args in [
            &["--profile", "profiling"][..],
            &["--profile=profiling"],
            &["--release"],
        ] {
            let args = strings(args);
            assert_eq!(Options::parse(&args).unwrap().cargo_args, args);
        }
    }

    #[test]
    fn normalizes_short_release_flag_for_bundler() {
        assert_eq!(
            Options::parse(&strings(&["-r"])).unwrap().cargo_args,
            ["--release"]
        );
    }

    #[test]
    fn discovers_plugins_and_excludes_support_crates() {
        let metadata = br#"{"packages":[
            {"name":"z-plugin","targets":[{"crate_types":["cdylib","lib"]},{"crate_types":["bin"]}]},
            {"name":"widgets","targets":[{"crate_types":["lib"]}]},
            {"name":"xtask","targets":[{"crate_types":["bin"]}]},
            {"name":"a-new-plugin","targets":[{"crate_types":["cdylib"]}]}
        ]}"#;
        assert_eq!(
            plugin_names(metadata).unwrap(),
            ["a-new-plugin", "z-plugin"]
        );
    }

    #[test]
    fn rejects_workspace_without_plugins() {
        assert!(plugin_names(br#"{"packages":[]}"#).is_err());
    }

    #[test]
    fn redistributable_replaces_cpu_and_feature_overrides() {
        let flags = strings(&[
            "-C",
            "target-cpu=native",
            "-Ctarget-feature=+avx2",
            "-C",
            "link-arg=-fuse-ld=mold",
        ]);
        assert_eq!(
            cpu_flags(&flags, "generic"),
            ["-C", "link-arg=-fuse-ld=mold", "-C", "target-cpu=generic"]
        );
    }

    #[test]
    fn native_replaces_compact_cpu_and_split_feature_overrides() {
        let flags = strings(&[
            "-Ctarget-cpu=generic",
            "-C",
            "target-feature=+avx512f",
            "--cfg",
            "custom",
        ]);
        assert_eq!(
            cpu_flags(&flags, "haswell"),
            ["--cfg", "custom", "-C", "target-cpu=haswell"]
        );
    }

    #[test]
    fn replaces_long_codegen_cpu_and_feature_overrides() {
        let flags = strings(&[
            "--codegen=target-cpu=native",
            "--codegen",
            "target-feature=+avx2",
        ]);
        assert_eq!(cpu_flags(&flags, "generic"), ["-C", "target-cpu=generic"]);
    }
}
