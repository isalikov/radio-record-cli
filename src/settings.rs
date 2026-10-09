use crate::error::{Error, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub favorites: BTreeSet<i64>,
    pub volume: u8,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            favorites: BTreeSet::new(),
            volume: 65,
        }
    }
}

impl Settings {
    pub fn path() -> Result<PathBuf> {
        if let Some(path) = std::env::var_os("RADIO_RECORD_CONFIG_DIR") {
            return Ok(PathBuf::from(path).join("settings.json"));
        }
        if let Some(path) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(path).join("radio-record/settings.json"));
        }
        let home = std::env::var_os("HOME").ok_or_else(|| Error::new("HOME is not set"))?;
        Ok(PathBuf::from(home).join(".config/radio-record/settings.json"))
    }

    pub fn load(path: &Path) -> Result<Self> {
        match fs::read(path) {
            Ok(bytes) => {
                let mut settings: Self = serde_json::from_slice(&bytes)
                    .map_err(|err| Error::new(format!("Settings: {err}")))?;
                settings.volume = settings.volume.min(100);
                Ok(settings)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Error::new("Missing settings directory"))?;
        fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut file, self).map_err(|err| Error::new(err.to_string()))?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|err| Error::new(err.to_string()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_survive_restart_and_replace_atomically() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/settings.json");
        let mut settings = Settings::load(&path).unwrap();
        settings.favorites.extend([12, 34]);
        settings.volume = 35;
        settings.save(&path).unwrap();
        settings.favorites.remove(&12);
        settings.save(&path).unwrap();
        let loaded = Settings::load(&path).unwrap();
        assert_eq!(loaded.favorites, BTreeSet::from([34]));
        assert_eq!(loaded.volume, 35);
    }
    #[test]
    fn corrupt_settings_are_reported_instead_of_silently_overwritten() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        fs::write(&path, "broken").unwrap();
        assert!(Settings::load(&path).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), "broken");
    }
}
