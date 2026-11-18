use super::{CurveLibrary, NEXT_WRITE};
use crate::mseg::{CurveShape, MAX_SHAPE_BYTES};
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};

pub const MAX_USER_PRESETS: usize = 256;
pub const MAX_USER_PRESET_NAME_CHARS: usize = 64;
const MAX_USER_PRESET_BYTES: usize = (MAX_SHAPE_BYTES + 1024) * MAX_USER_PRESETS + 4096;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserPreset {
    pub id: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub category: String,
    pub shape: CurveShape,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct UserBank {
    version: u32,
    next_id: u64,
    presets: Vec<UserPreset>,
}

impl Default for UserBank {
    fn default() -> Self {
        Self {
            version: 1,
            next_id: 1,
            presets: Vec::new(),
        }
    }
}

fn name(value: &str) -> io::Result<String> {
    let trimmed = value.trim();
    if value.chars().any(char::is_control)
        || trimmed.is_empty()
        || trimmed.chars().count() > MAX_USER_PRESET_NAME_CHARS
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Use a name of 1–64 characters without controls",
        ));
    }
    Ok(trimmed.to_owned())
}

fn category(value: &str) -> io::Result<String> {
    let trimmed = value.trim();
    if value.chars().any(char::is_control) || trimmed.chars().count() > MAX_USER_PRESET_NAME_CHARS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Use an empty category or 1–64 characters without controls",
        ));
    }
    Ok(trimmed.to_owned())
}

impl UserBank {
    fn validate(&self) -> io::Result<()> {
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        if !matches!(self.version, 1 | 2)
            || self.next_id == 0
            || self.presets.len() > MAX_USER_PRESETS
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid user preset library",
            ));
        }
        for preset in &self.presets {
            if preset.id == 0
                || preset.id >= self.next_id
                || !ids.insert(preset.id)
                || name(&preset.name).ok().as_deref() != Some(preset.name.as_str())
                || category(&preset.category).ok().as_deref() != Some(preset.category.as_str())
                || (self.version == 1 && !preset.category.is_empty())
                || !names.insert(preset.name.to_lowercase())
            {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid user preset record",
                ));
            }
        }
        Ok(())
    }

    fn unique_name(&self, name: &str, except: Option<u64>) -> io::Result<()> {
        let key = name.to_lowercase();
        if self
            .presets
            .iter()
            .any(|preset| Some(preset.id) != except && preset.name.to_lowercase() == key)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "Name already exists; choose another name",
            ));
        }
        Ok(())
    }

    fn expected(&self, expected: &UserPreset) -> io::Result<usize> {
        self.presets
            .iter()
            .position(|live| live == expected)
            .ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "Preset changed; refresh and choose again",
                )
            })
    }
}

impl CurveLibrary {
    fn read_user_bank(&self) -> io::Result<UserBank> {
        let file = match File::open(self.root.join("user-presets.json")) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(UserBank::default()),
            Err(error) => return Err(error),
        };
        let mut text = String::new();
        file.take(MAX_USER_PRESET_BYTES as u64 + 1)
            .read_to_string(&mut text)?;
        if text.len() > MAX_USER_PRESET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "User preset library is too large",
            ));
        }
        let bank: UserBank = serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        bank.validate()?;
        Ok(bank)
    }

    /// Read a GUI-side snapshot ordered by name, without creating missing files.
    pub fn user_presets(&self) -> io::Result<Vec<UserPreset>> {
        let mut presets = self.read_user_bank()?.presets;
        presets.sort_by_cached_key(|preset| {
            (preset.name.to_lowercase(), preset.name.clone(), preset.id)
        });
        Ok(presets)
    }

    fn update_user_bank<T>(
        &self,
        edit: impl FnOnce(&mut UserBank) -> io::Result<T>,
    ) -> io::Result<T> {
        fs::create_dir_all(&self.root)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.root.join("user-presets.lock"))?;
        lock.try_lock().map_err(io::Error::from)?;
        let mut bank = self.read_user_bank()?;
        let result = edit(&mut bank)?;
        if bank
            .presets
            .iter()
            .any(|preset| !preset.category.is_empty())
        {
            bank.version = 2;
        }
        let text = serde_json::to_string_pretty(&bank).map_err(io::Error::other)?;
        if text.len() > MAX_USER_PRESET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "User preset library is too large",
            ));
        }
        Self::write_atomic(&self.root.join("user-presets.json"), &text, &NEXT_WRITE)?;
        Ok(result)
    }

    pub fn save_user_preset(&self, value: &str, shape: &CurveShape) -> io::Result<UserPreset> {
        self.save_user_preset_with_category(value, "", shape)
    }

    pub fn save_user_preset_with_category(
        &self,
        value: &str,
        label: &str,
        shape: &CurveShape,
    ) -> io::Result<UserPreset> {
        let name = name(value)?;
        let category = category(label)?;
        crate::mseg::shape::validate_nodes(shape.nodes())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidInput, error))?;
        self.update_user_bank(|bank| {
            bank.unique_name(&name, None)?;
            if bank.presets.len() >= MAX_USER_PRESETS {
                return Err(io::Error::other(
                    "User preset library is full (256 presets)",
                ));
            }
            let next = bank
                .next_id
                .checked_add(1)
                .ok_or_else(|| io::Error::other("User preset identities exhausted"))?;
            let preset = UserPreset {
                id: bank.next_id,
                name,
                category,
                shape: shape.clone(),
            };
            bank.next_id = next;
            bank.presets.push(preset.clone());
            Ok(preset)
        })
    }

    pub fn load_user_preset(&self, expected: &UserPreset) -> io::Result<CurveShape> {
        let bank = self.read_user_bank()?;
        let index = bank.expected(expected)?;
        Ok(bank.presets[index].shape.clone())
    }

    pub fn rename_user_preset(&self, expected: &UserPreset, value: &str) -> io::Result<UserPreset> {
        self.update_user_preset(expected, value, &expected.category)
    }

    pub fn update_user_preset(
        &self,
        expected: &UserPreset,
        value: &str,
        label: &str,
    ) -> io::Result<UserPreset> {
        let name = name(value)?;
        let category = category(label)?;
        self.update_user_bank(|bank| {
            let index = bank.expected(expected)?;
            bank.unique_name(&name, Some(expected.id))?;
            bank.presets[index].name = name;
            bank.presets[index].category = category;
            Ok(bank.presets[index].clone())
        })
    }

    pub fn remove_user_preset(&self, expected: &UserPreset) -> io::Result<()> {
        self.update_user_bank(|bank| {
            let index = bank.expected(expected)?;
            bank.presets.remove(index);
            Ok(())
        })
    }
}

#[cfg(test)]
mod category_tests;
#[cfg(test)]
mod tests;
