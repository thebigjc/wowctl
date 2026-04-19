//! Configuration management for wowctl.
//!
//! Handles loading and saving configuration from platform-specific locations,
//! auto-detecting addon directories, and resolving API keys from config or environment.

use crate::addon::ReleaseChannel;
use crate::error::{Result, WowctlError};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod embedded_key {
    include!(concat!(env!("OUT_DIR"), "/embedded_key.rs"));
}

/// Main configuration structure for wowctl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    /// Path to the WoW addon directory. If None, will be auto-detected.
    pub addon_dir: Option<PathBuf>,
    /// CurseForge API key. Can also be set via WOWCTL_CURSEFORGE_API_KEY env var.
    pub curseforge_api_key: Option<String>,
    /// Whether to use colored output.
    #[serde(default = "default_color")]
    pub color: bool,
    /// Default release channel for addon installs/updates (stable, beta, alpha).
    #[serde(default)]
    pub default_release_channel: Option<ReleaseChannel>,
}

fn default_color() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        Self {
            addon_dir: None,
            curseforge_api_key: None,
            color: true,
            default_release_channel: None,
        }
    }
}

impl Config {
    /// Returns the path to the configuration file.
    pub fn config_path() -> Result<PathBuf> {
        let config_dir = dirs::config_dir().ok_or_else(|| {
            WowctlError::Config("Could not determine config directory".to_string())
        })?;
        Ok(config_dir.join("wowctl").join("config.toml"))
    }

    /// Returns the path to the data directory where registry and other data is stored.
    pub fn data_dir() -> Result<PathBuf> {
        let data_dir = dirs::data_local_dir()
            .ok_or_else(|| WowctlError::Config("Could not determine data directory".to_string()))?;
        Ok(data_dir.join("wowctl"))
    }

    /// Loads configuration from disk. Returns default config if file doesn't exist.
    pub fn load() -> Result<Self> {
        let config_path = Self::config_path()?;

        if !config_path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&config_path)
            .map_err(|e| WowctlError::Config(format!("Failed to read config file: {e}")))?;

        let config: Config = toml::from_str(&contents)?;
        Ok(config)
    }

    /// Saves configuration to disk atomically (write to temp file, then rename).
    pub fn save(&self) -> Result<()> {
        let config_path = Self::config_path()?;

        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self)?;

        let temp_path = config_path.with_extension("toml.tmp");
        std::fs::write(&temp_path, contents)?;
        std::fs::rename(temp_path, config_path)?;

        Ok(())
    }

    /// Gets the CurseForge API key.
    /// Precedence: environment variable > config file > embedded build-time key.
    pub fn get_api_key(&self) -> Result<String> {
        if let Ok(key) = std::env::var("WOWCTL_CURSEFORGE_API_KEY") {
            return Ok(key);
        }

        if let Some(ref key) = self.curseforge_api_key {
            return Ok(key.clone());
        }

        if let Some(key) = embedded_key::embedded_api_key() {
            return Ok(key);
        }

        Err(WowctlError::MissingApiKey(
            "CurseForge API key not found. Run 'wowctl config init' or set WOWCTL_CURSEFORGE_API_KEY environment variable".to_string()
        ))
    }

    /// Resolves the release channel: CLI override > config default > Stable.
    pub fn resolve_channel(&self, cli_override: Option<ReleaseChannel>) -> ReleaseChannel {
        cli_override
            .or(self.default_release_channel)
            .unwrap_or_default()
    }

    /// Gets the addon directory path, checking for override, config, then auto-detection.
    pub fn get_addon_dir(&self) -> Result<PathBuf> {
        if let Ok(override_dir) = std::env::var("WOWCTL_ADDON_DIR_OVERRIDE") {
            return Ok(PathBuf::from(override_dir));
        }

        if let Some(ref dir) = self.addon_dir {
            return Ok(dir.clone());
        }

        Self::detect_addon_dir()
    }

    /// Attempts to auto-detect the WoW addon directory for the current platform.
    pub fn detect_addon_dir() -> Result<PathBuf> {
        #[cfg(target_os = "macos")]
        {
            let path = PathBuf::from("/Applications/World of Warcraft/_retail_/Interface/AddOns");
            if path.exists() {
                return Ok(path);
            }
        }

        #[cfg(target_os = "windows")]
        {
            let path = PathBuf::from(
                r"C:\Program Files (x86)\World of Warcraft\_retail_\Interface\AddOns",
            );
            if path.exists() {
                return Ok(path);
            }
        }

        #[cfg(target_os = "linux")]
        {
            // WSL: Windows drives are mounted under /mnt/<letter>/
            let path = PathBuf::from(
                "/mnt/c/Program Files (x86)/World of Warcraft/_retail_/Interface/AddOns",
            );
            if path.exists() {
                return Ok(path);
            }
        }

        Err(WowctlError::InvalidAddonDir(
            "Could not auto-detect WoW addon directory. Please set it manually with 'wowctl config set addon_dir <path>'".to_string()
        ))
    }
}
