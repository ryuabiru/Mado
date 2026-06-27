use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::{info, warn};

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub font: FontConfig,
    pub window: WindowConfig,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FontConfig {
    pub family: String,
    pub size: f32,
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: "HackGen Console NF".to_owned(),
            size: 15.0,
        }
    }
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            width: 960,
            height: 640,
        }
    }
}

impl Config {
    pub fn load(explicit_path: Option<&Path>) -> Self {
        let path = explicit_path
            .map(Path::to_owned)
            .or_else(default_config_path);
        let Some(path) = path else {
            return Self::default();
        };
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(error) => {
                warn!(path = %path.display(), %error, "failed to read Mado settings; using defaults");
                return Self::default();
            }
        };
        match toml::from_str::<Self>(&source) {
            Ok(config) if config.is_valid() => {
                info!(path = %path.display(), "loaded Mado settings");
                config
            }
            Ok(_) => {
                warn!(path = %path.display(), "invalid Mado settings values; using defaults");
                Self::default()
            }
            Err(error) => {
                warn!(path = %path.display(), %error, "invalid Mado settings file; using defaults");
                Self::default()
            }
        }
    }

    fn is_valid(&self) -> bool {
        !self.font.family.trim().is_empty()
            && self.font.size.is_finite()
            && (6.0..=72.0).contains(&self.font.size)
            && (320..=16_384).contains(&self.window.width)
            && (200..=16_384).contains(&self.window.height)
    }
}

pub fn default_config_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        env::var_os("HOME")
            .map(PathBuf::from)
            .map(|home| home.join("Library/Application Support/Mado/config.toml"))
    }
    #[cfg(target_os = "windows")]
    {
        env::var_os("APPDATA")
            .map(PathBuf::from)
            .map(|base| base.join("Mado").join("config.toml"))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
            .map(|base| base.join("mado").join("config.toml"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_partial_settings_over_defaults() {
        let config: Config = toml::from_str(
            r#"
                [font]
                size = 18.0

                [window]
                width = 1200
            "#,
        )
        .unwrap();
        assert_eq!(config.font.family, "HackGen Console NF");
        assert_eq!(config.font.size, 18.0);
        assert_eq!(config.window.width, 1200);
        assert_eq!(config.window.height, 640);
    }

    #[test]
    fn validates_safe_ranges() {
        let mut config = Config::default();
        assert!(config.is_valid());
        config.font.size = f32::NAN;
        assert!(!config.is_valid());
        config = Config::default();
        config.window.width = 1;
        assert!(!config.is_valid());
    }
}
