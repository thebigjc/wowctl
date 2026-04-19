//! Local addon registry management.
//!
//! The registry tracks all addons managed by wowctl, including their versions,
//! directories, and dependency relationships.

use crate::addon::InstalledAddon;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Registry of installed addons. Maps addon slug to addon metadata.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Registry {
    pub addons: HashMap<String, InstalledAddon>,
}

impl Registry {
    /// Returns the path to the registry file.
    pub fn registry_path() -> Result<PathBuf> {
        let data_dir = Config::data_dir()?;
        Ok(data_dir.join("registry.toml"))
    }

    /// Loads the registry from disk. Returns empty registry if file doesn't exist.
    pub fn load() -> Result<Self> {
        let registry_path = Self::registry_path()?;

        if !registry_path.exists() {
            return Ok(Self::default());
        }

        let contents = std::fs::read_to_string(&registry_path)
            .map_err(|e| WowctlError::Registry(format!("Failed to read registry: {e}")))?;

        let registry: Registry = toml::from_str(&contents)
            .map_err(|e| WowctlError::Registry(format!("Failed to parse registry: {e}")))?;

        Ok(registry)
    }

    /// Saves the registry to disk atomically.
    pub fn save(&self) -> Result<()> {
        let registry_path = Self::registry_path()?;

        if let Some(parent) = registry_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let contents = toml::to_string_pretty(self)?;

        let temp_path = registry_path.with_extension("toml.tmp");
        std::fs::write(&temp_path, contents)?;
        std::fs::rename(temp_path, registry_path)?;

        Ok(())
    }

    /// Gets an addon by slug.
    pub fn get(&self, slug: &str) -> Option<&InstalledAddon> {
        self.addons.get(slug)
    }

    /// Adds or updates an addon in the registry.
    pub fn add(&mut self, addon: InstalledAddon) {
        self.addons.insert(addon.slug.clone(), addon);
    }

    /// Removes an addon from the registry and returns it.
    pub fn remove(&mut self, slug: &str) -> Option<InstalledAddon> {
        self.addons.remove(slug)
    }

    /// Returns a list of all installed addons.
    pub fn list_all(&self) -> Vec<&InstalledAddon> {
        self.addons.values().collect()
    }

    /// Finds an addon that owns the specified directory.
    pub fn find_by_directory(&self, directory: &str) -> Option<&InstalledAddon> {
        self.addons
            .values()
            .find(|addon| addon.directories.contains(&directory.to_string()))
    }

    /// Finds all dependencies that are no longer required by any addon.
    pub fn find_orphaned_dependencies(&self) -> Vec<String> {
        let mut orphans = Vec::new();

        for (slug, addon) in &self.addons {
            if addon.is_dependency && addon.required_by.is_empty() {
                orphans.push(slug.clone());
            }
        }

        orphans
    }

    /// Updates all addons' required_by lists to remove references to a removed addon.
    pub fn update_dependency_references(&mut self, removed_slug: &str) {
        for addon in self.addons.values_mut() {
            addon.required_by.retain(|s| s != removed_slug);
        }
    }
}
