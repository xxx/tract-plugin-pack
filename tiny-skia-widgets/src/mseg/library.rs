//! Shared, GUI-only curve clipboard and favorite storage.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::{CurveShape, MAX_SHAPE_BYTES};

pub const FAVORITE_COUNT: usize = 8;
static NEXT_WRITE: AtomicU64 = AtomicU64::new(0);

pub struct CurveLibrary {
    root: PathBuf,
}

impl CurveLibrary {
    pub fn user_library() -> io::Result<Self> {
        let dirs = directories::ProjectDirs::from("audio", "Tract", "Tract Plugin Pack")
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "User data directory unavailable")
            })?;
        Ok(Self::at(dirs.data_dir().join("curves")))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn copy(&self, shape: &CurveShape) -> io::Result<()> {
        Self::export(&self.root.join("clipboard.json"), shape)
    }

    pub fn paste(&self) -> io::Result<CurveShape> {
        Self::import(&self.root.join("clipboard.json"))
    }

    pub fn store_favorite(&self, slot: usize, shape: &CurveShape) -> io::Result<()> {
        Self::export(&self.favorite_path(slot)?, shape)
    }

    pub fn load_favorite(&self, slot: usize) -> io::Result<CurveShape> {
        Self::import(&self.favorite_path(slot)?)
    }

    fn favorite_path(&self, slot: usize) -> io::Result<PathBuf> {
        if slot >= FAVORITE_COUNT {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Invalid favorite slot",
            ));
        }
        Ok(self.root.join(format!("favorite-{}.json", slot + 1)))
    }

    pub fn import(path: &Path) -> io::Result<CurveShape> {
        let file = File::open(path)?;
        let mut text = String::new();
        file.take(MAX_SHAPE_BYTES as u64 + 1)
            .read_to_string(&mut text)?;
        CurveShape::from_json(&text).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }

    pub fn export(path: &Path, shape: &CurveShape) -> io::Result<()> {
        let text = shape.to_json().map_err(io::Error::other)?;
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            fs::create_dir_all(parent)?;
        }
        Self::write_atomic(path, &text, &NEXT_WRITE)
    }

    fn write_atomic(path: &Path, text: &str, counter: &AtomicU64) -> io::Result<()> {
        for _ in 0..64 {
            let serial = counter.fetch_add(1, Ordering::Relaxed);
            let mut temp_name = path.as_os_str().to_os_string();
            temp_name.push(format!(".{}.{}.tmp", std::process::id(), serial));
            let temp = PathBuf::from(temp_name);
            let mut file = match OpenOptions::new().write(true).create_new(true).open(&temp) {
                Ok(file) => file,
                Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(err) => return Err(err),
            };
            let result = (|| {
                file.write_all(text.as_bytes())?;
                file.sync_all()?;
                drop(file);
                fs::rename(&temp, path)
            })();
            if result.is_err() {
                let _ = fs::remove_file(temp);
            }
            return result;
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Curve storage busy; try again",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mseg::{FactoryShape, factory_shape};

    #[test]
    fn atomic_write_retries_collisions_without_removing_another_writers_file() {
        let root = std::env::temp_dir().join(format!("tract-curve-write-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("curve.json");
        let other = root.join(format!("curve.json.{}.0.tmp", std::process::id()));
        fs::write(&other, "other writer").unwrap();
        CurveLibrary::write_atomic(&path, "our curve", &AtomicU64::new(0)).unwrap();
        assert_eq!(fs::read_to_string(&other).unwrap(), "other writer");
        assert_eq!(fs::read_to_string(&path).unwrap(), "our curve");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn different_library_instances_share_clipboard_and_favorites() {
        let root = std::env::temp_dir().join(format!(
            "tract-curves-{}-{}",
            std::process::id(),
            NEXT_WRITE.fetch_add(1, Ordering::Relaxed)
        ));
        let first = CurveLibrary::at(root.clone());
        let second = CurveLibrary::at(root.clone());
        assert_eq!(first.paste().unwrap_err().kind(), io::ErrorKind::NotFound);
        let shape = factory_shape(FactoryShape::Triangle);
        first.copy(&shape).unwrap();
        assert_eq!(second.paste().unwrap(), shape);
        first.store_favorite(3, &shape).unwrap();
        assert_eq!(second.load_favorite(3).unwrap(), shape);
        assert!(first.store_favorite(8, &shape).is_err());
        let replacement = factory_shape(FactoryShape::Duck);
        second.copy(&replacement).unwrap();
        assert_eq!(first.paste().unwrap(), replacement);
        fs::write(root.join("clipboard.json"), "bad data").unwrap();
        assert_eq!(
            second.paste().unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
        fs::remove_dir_all(root).unwrap();
    }
}
