"""Package the workspace's bundled plugins and user manuals by format."""

import argparse
import json
from pathlib import Path
import re
import subprocess
from zipfile import ZIP_DEFLATED, ZipFile


def files_in(directory):
    return sorted(path for path in directory.rglob("*") if path.is_file())


def cargo_metadata(*options):
    return json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", *options],
        text=True,
        encoding="utf-8",
    ))


def dependency_notices(metadata, plugin_names, upstream):
    packages = {package["id"]: package for package in metadata["packages"]}
    nodes = {node["id"]: node for node in metadata["resolve"]["nodes"]}
    pending = [package_id for package_id in metadata["workspace_members"]
               if packages[package_id]["name"] in plugin_names]
    visited = set()
    notices = {}
    while pending:
        package_id = pending.pop()
        if package_id in visited:
            continue
        visited.add(package_id)
        pending.extend(dep["pkg"] for dep in nodes[package_id]["deps"]
                       if any(kind["kind"] is None for kind in dep["dep_kinds"]))
        package = packages[package_id]
        if package["source"] is None:
            continue

        name, version = package["name"], package["version"]
        manifest_dir = Path(package["manifest_path"]).parent
        files = {path for path in manifest_dir.iterdir() if path.is_file()
                 and path.name.lower().startswith(("license", "licence", "copying",
                                                   "copyright", "notice", "unlicense"))}
        if package["license_file"]:
            files.add(manifest_dir / package["license_file"])
        if not files and name == "nih_plug_derive":
            files.add(manifest_dir.parent / "LICENSE")
        if not files:
            fallback = "objc2" if package["repository"] == "https://github.com/madsmtm/objc2" else name
            files.update(upstream.glob(f"{fallback}.*"))
        if not files:
            raise SystemExit(f"Missing license notices for {name} {version}")
        for path in sorted(files):
            notices[f"licenses/cargo/{name}-{version}/{path.name}"] = path
    return notices


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("prefix", help="ZIP filename before the format suffix")
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--target", action="append", help="Rust target triple (defaults to the host)")
    args = parser.parse_args()
    prefix = re.sub(r"[^a-zA-Z0-9._-]", "-", args.prefix)

    metadata = cargo_metadata("--no-deps")
    plugins = sorted(
        package["name"] for package in metadata["packages"]
        if any("cdylib" in target["crate_types"] for target in package["targets"])
    )
    if not plugins:
        raise SystemExit("No plugin crates found in the workspace")

    root = Path(metadata["workspace_root"])
    docs = root / "docs"
    bundled = Path(metadata["target_directory"]) / "bundled"
    targets = args.target
    if not targets:
        rustc_version = subprocess.check_output(["rustc", "-vV"], text=True)
        targets = [line.removeprefix("host: ") for line in rustc_version.splitlines()
                   if line.startswith("host: ")]
        if not targets:
            raise SystemExit("Could not determine the Rust host triple")
    notices = {}
    for target in targets:
        notices.update(dependency_notices(
            cargo_metadata("--filter-platform", target), plugins, root / "licenses/cargo-upstream",
        ))

    documentation = [
        root / "LICENSE",
        root / "licenses/README.md",
        root / "licenses/DejaVu.txt",
        root / "gated/THIRD_PARTY_NOTICES.md",
        root / "gated/licenses/Apache-2.0.txt",
        docs / "installing-macos.md",
        docs / "mseg-editor.md",
        docs / "mseg-editor.pdf",
    ]
    screenshots = files_in(docs / "mseg")
    if not screenshots:
        raise SystemExit("No MSEG editor screenshots found")
    documentation.extend(screenshots)
    for plugin in plugins:
        manual_dir = docs / plugin
        for extension in ("md", "pdf"):
            manual = manual_dir / f"{plugin}-manual.{extension}"
            if not manual.is_file():
                raise SystemExit(f"Missing plugin manual: {manual}")
        documentation.extend(files_in(manual_dir))
    for path in documentation:
        if not path.is_file():
            raise SystemExit(f"Missing release documentation: {path}")

    args.output_dir.mkdir(parents=True, exist_ok=True)
    for plugin_format in ("clap", "vst3"):
        archive_path = args.output_dir / f"{prefix}-{plugin_format}.zip"
        plugin_files = []
        for plugin in plugins:
            bundle = bundled / f"{plugin}.{plugin_format}"
            files = [bundle] if bundle.is_file() else files_in(bundle)
            if not files:
                raise SystemExit(f"Missing or empty plugin bundle: {bundle}")
            plugin_files.extend(files)

        with ZipFile(archive_path, "w", compression=ZIP_DEFLATED, strict_timestamps=False) as archive:
            for path in plugin_files:
                archive.write(path, path.relative_to(bundled).as_posix())
            for path in documentation:
                archive.write(path, path.relative_to(root).as_posix())
            for entry, path in sorted(notices.items()):
                archive.write(path, entry)
        print(f"Packaged {len(plugins)} plugins in {archive_path}")


if __name__ == "__main__":
    main()
