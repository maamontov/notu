use std::{collections::BTreeMap, fs, io::Write, path::Path};

use anyhow::{Context, Result, ensure};
use ratatui::style::Color;
use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

pub const DAILY_TAG: &str = "daily";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Accent {
    #[default]
    Cyan,
    Blue,
    Green,
    Magenta,
    Yellow,
    Red,
    White,
}

impl Accent {
    pub const ALL: [Self; 7] = [
        Self::Cyan,
        Self::Blue,
        Self::Green,
        Self::Magenta,
        Self::Yellow,
        Self::Red,
        Self::White,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cyan => "Cyan",
            Self::Blue => "Blue",
            Self::Green => "Green",
            Self::Magenta => "Magenta",
            Self::Yellow => "Yellow",
            Self::Red => "Red",
            Self::White => "White",
        }
    }

    pub fn color(self) -> Color {
        match self {
            Self::Cyan => Color::Cyan,
            Self::Blue => Color::LightBlue,
            Self::Green => Color::Green,
            Self::Magenta => Color::LightMagenta,
            Self::Yellow => Color::Yellow,
            Self::Red => Color::LightRed,
            Self::White => Color::White,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub accent: Accent,
    pub tags: BTreeMap<String, Accent>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            accent: Accent::default(),
            tags: BTreeMap::from([(DAILY_TAG.to_owned(), Accent::default())]),
        }
    }
}

impl Config {
    pub fn load_recovering(workspace: &Path) -> Result<(Self, Option<String>)> {
        let path = workspace.join(".notu.json");
        match Self::load(workspace) {
            Ok(config) => Ok((config, None)),
            Err(error) => {
                // I/O failures must remain errors: only malformed content can be rebuilt.
                if error.downcast_ref::<std::io::Error>().is_some() {
                    return Err(error);
                }
                let bytes = fs::read(&path)?;
                let warning = crate::recovery::backup(&path, &bytes)?;
                let config = Self::default();
                config.save(workspace)?;
                Ok((config, Some(warning)))
            }
        }
    }

    pub fn load(workspace: &Path) -> Result<Self> {
        let path = workspace.join(".notu.json");
        match fs::read(&path) {
            Ok(bytes) => {
                let mut config: Self = serde_json::from_slice(&bytes)
                    .with_context(|| format!("Cannot parse settings {}", path.display()))?;
                for name in config.tags.keys() {
                    Self::validate_tag(name)?;
                }
                config.tags.entry(DAILY_TAG.to_owned()).or_default();
                Ok(config)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => {
                Err(error).with_context(|| format!("Cannot read settings {}", path.display()))
            }
        }
    }

    pub fn validate_tag(name: &str) -> Result<()> {
        ensure!(
            !name.is_empty()
                && name == name.trim()
                && name.len() <= 80
                && !name.chars().any(|c| c == ':' || c.is_control()),
            "Use a tag of 1–80 bytes without colons, control characters or outer spaces"
        );
        Ok(())
    }

    pub fn save(&self, workspace: &Path) -> Result<()> {
        ensure!(
            self.tags.contains_key(DAILY_TAG),
            "The daily tag is built in and cannot be removed"
        );
        for name in self.tags.keys() {
            Self::validate_tag(name)?;
        }
        let mut file = NamedTempFile::new_in(workspace)?;
        serde_json::to_writer_pretty(&mut file, self)?;
        file.write_all(b"\n")?;
        file.as_file().sync_all()?;
        file.persist(workspace.join(".notu.json"))
            .map_err(|error| error.error)
            .context("Cannot save settings")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_and_persistent_accent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        assert_eq!(Config::load(dir.path())?, Config::default());
        let config = Config {
            accent: Accent::Magenta,
            ..Config::default()
        };
        config.save(dir.path())?;
        assert_eq!(Config::load(dir.path())?, config);
        fs::write(dir.path().join(".notu.json"), "invalid")?;
        assert!(Config::load(dir.path()).is_err());
        Ok(())
    }

    #[test]
    fn old_settings_load_and_unicode_tags_persist() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(dir.path().join(".notu.json"), r#"{"accent":"blue"}"#)?;
        let mut config = Config::load(dir.path())?;
        assert_eq!(config.accent, Accent::Blue);
        assert_eq!(config.tags, Config::default().tags);
        config.tags.insert("Работа".into(), Accent::Yellow);
        config.tags.insert("asd".into(), Accent::Red);
        config.save(dir.path())?;
        assert_eq!(Config::load(dir.path())?, config);
        for name in ["", "a:b", "a\nb", " a", "a "] {
            assert!(Config::validate_tag(name).is_err());
        }
        Ok(())
    }

    #[test]
    fn damaged_settings_are_backed_up_and_rebuilt_without_touching_notes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let broken = b"{broken settings";
        fs::write(dir.path().join(".notu.json"), broken)?;
        fs::write(dir.path().join("Keep.md"), "Body")?;
        let (config, warning) = Config::load_recovering(dir.path())?;
        assert_eq!(config, Config::default());
        assert!(warning.unwrap().starts_with("Warning:"));
        let backup = fs::read_dir(dir.path())?
            .filter_map(|entry| entry.ok())
            .find(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with(".notu.json.corrupt-")
            })
            .unwrap();
        assert_eq!(fs::read(backup.path())?, broken);
        assert_eq!(fs::read_to_string(dir.path().join("Keep.md"))?, "Body");
        assert!(Config::load_recovering(dir.path())?.1.is_none());
        Ok(())
    }

    #[test]
    fn built_in_daily_is_restored_without_resetting_its_color() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs::write(
            dir.path().join(".notu.json"),
            r#"{"accent":"blue","tags":{"work":"red"}}"#,
        )?;
        let mut config = Config::load(dir.path())?;
        assert_eq!(config.tags[DAILY_TAG], Accent::Cyan);
        assert_eq!(config.tags["work"], Accent::Red);
        config.tags.insert(DAILY_TAG.into(), Accent::Yellow);
        config.save(dir.path())?;
        assert_eq!(Config::load(dir.path())?.tags[DAILY_TAG], Accent::Yellow);
        config.tags.remove(DAILY_TAG);
        assert!(config.save(dir.path()).is_err());
        assert_eq!(Config::load(dir.path())?.tags[DAILY_TAG], Accent::Yellow);
        Ok(())
    }
}
