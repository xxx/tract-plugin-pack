# Third-party notices

- `cargo/` in a release ZIP contains license and notice files from the locked
  Rust dependencies for that build. Universal macOS ZIPs include the notices
  for both architectures.
- `DejaVu.txt` covers the DejaVu typefaces embedded in the plug-ins and manuals.
  [DejaVu 2.37 license](https://github.com/dejavu-fonts/dejavu-fonts/blob/version_2_37/LICENSE)
- `../gated/THIRD_PARTY_NOTICES.md` credits the factory rhythm data. Its Apache
  license is included under `../gated/licenses/`.

## Upstream copies

Some published Rust crates omit their upstream license files. The packager
uses these copies when the crate lacks its own file. Review them when updating
the corresponding dependencies.

- com-scrape-types 0.1.1. [vst3-rs license](https://github.com/coupler-rs/vst3-rs/blob/33f1859fbdcfa247c1fdd17d565cb5285860e4ce/LICENSE-MIT)
- dasp_sample 0.11.0. [dasp license](https://github.com/RustAudio/dasp/blob/97c3bb9b2363c0b46ac1633858bf1054fd02a980/LICENSE-MIT)
- drm-fourcc 2.2.0. [drm-fourcc-rs license](https://github.com/danielzfranklin/drm-fourcc-rs/blob/1ad1a978a9aaa78ff0016cc7013787e4d1d68682/LICENSE)
- malloc_buf 0.0.6. [malloc_buf license](https://github.com/SSheldon/malloc_buf/blob/d9a3e539642bd90e07df458d226b19cdfa606863/LICENSE)
- objc2 and related crates. [objc2 licensing notes](https://github.com/madsmtm/objc2/blob/b4167b582b2f75f9a1be75495c41b765344fd03c/LICENSE.md)
