//! Core addon data structures.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// Information about an addon from a source (e.g., CurseForge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddonInfo {
    pub id: String,
    pub name: String,
    pub slug: String,
    pub description: Option<String>,
    pub download_count: Option<u64>,
    pub source: String,
}

/// Paginated search results from an addon source.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub addons: Vec<AddonInfo>,
    pub page: u32,
    pub page_size: u32,
    pub total_count: u32,
}

impl SearchResult {
    pub fn total_pages(&self) -> u32 {
        if self.page_size == 0 {
            return 0;
        }
        self.total_count.div_ceil(self.page_size)
    }
}

/// Version information for a specific addon release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    pub file_id: u32,
    pub version: String,
    pub display_name: String,
    pub download_url: String,
    pub file_name: String,
    pub file_size: u64,
    pub game_versions: Vec<String>,
    pub released_at: String,
    pub dependencies: Vec<DependencyInfo>,
    /// Canonical directory names from CurseForge file modules.
    /// Each entry is a top-level folder this addon installs (e.g., `["WeakAuras", "WeakAurasOptions"]`).
    #[serde(default)]
    pub modules: Vec<String>,
}

/// Dependency information for an addon.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DependencyInfo {
    pub addon_id: String,
    pub dependency_type: DependencyType,
}

/// Type of dependency relationship.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DependencyType {
    Required,
    Optional,
    Embedded,
}

/// Release channel preference for addon files.
///
/// CurseForge uses `releaseType` integers: 1=Release, 2=Beta, 3=Alpha.
/// A channel preference of Beta means "accept Release and Beta files" —
/// i.e., any file whose `releaseType <= channel` is eligible.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    clap::ValueEnum,
    Default,
)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseChannel {
    #[default]
    Stable = 1,
    Beta = 2,
    Alpha = 3,
}

impl ReleaseChannel {
    /// Returns the CurseForge `releaseType` integer for this channel.
    pub fn as_cf_release_type(self) -> u32 {
        self as u32
    }

    /// Returns true if a file with the given CurseForge `releaseType` is
    /// acceptable under this channel preference.
    pub fn includes_release_type(self, release_type: u32) -> bool {
        release_type <= self.as_cf_release_type()
    }
}

impl fmt::Display for ReleaseChannel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Stable => write!(f, "stable"),
            Self::Beta => write!(f, "beta"),
            Self::Alpha => write!(f, "alpha"),
        }
    }
}

impl FromStr for ReleaseChannel {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stable" | "release" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "alpha" => Ok(Self::Alpha),
            _ => Err(format!(
                "unknown release channel '{}' (expected: stable, beta, alpha)",
                s
            )),
        }
    }
}

/// WoW game flavor, used to select the correct flavor-specific `.toc` file.
///
/// WoW addons may ship multiple TOC files with flavor suffixes
/// (e.g., `MyAddon_Mainline.toc`, `MyAddon_Classic.toc`). The client
/// loads the one matching the running game version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GameFlavor {
    #[default]
    Retail,
    Classic,
}

impl GameFlavor {
    /// Returns the TOC file suffix for this flavor (e.g., `_Mainline` for Retail).
    pub fn toc_suffix(self) -> &'static str {
        match self {
            GameFlavor::Retail => "_Mainline",
            GameFlavor::Classic => "_Classic",
        }
    }
}

/// An addon that has been installed and is tracked in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledAddon {
    pub name: String,
    pub slug: String,
    pub version: String,
    pub source: String,
    pub addon_id: String,
    pub directories: Vec<String>,
    pub is_dependency: bool,
    pub required_by: Vec<String>,
    #[serde(default)]
    pub installed_file_id: Option<u32>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub channel: Option<ReleaseChannel>,
    #[serde(default)]
    pub ignored: Option<bool>,
    #[serde(default)]
    pub game_versions: Option<Vec<String>>,
    #[serde(default)]
    pub released_at: Option<String>,
    #[serde(default)]
    pub auto_update: Option<bool>,
}

impl InstalledAddon {
    pub fn is_ignored(&self) -> bool {
        self.ignored.unwrap_or(false)
    }

    pub fn is_auto_update(&self) -> bool {
        self.auto_update.unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_addon_backward_compat() {
        let toml_str = r#"
            name = "WeakAuras"
            slug = "weakauras-2"
            version = "5.12.8"
            source = "curseforge"
            addon_id = "65387"
            directories = ["WeakAuras", "WeakAurasOptions"]
            is_dependency = false
            required_by = []
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.name, "WeakAuras");
        assert_eq!(addon.installed_file_id, None);
        assert_eq!(addon.display_name, None);
        assert_eq!(addon.channel, None);
        assert_eq!(addon.ignored, None);
        assert_eq!(addon.game_versions, None);
        assert_eq!(addon.released_at, None);
        assert_eq!(addon.auto_update, None);
        assert!(!addon.is_ignored());
        assert!(!addon.is_auto_update());
    }

    #[test]
    fn installed_addon_with_new_fields() {
        let toml_str = r#"
            name = "WeakAuras"
            slug = "weakauras-2"
            version = "5.12.8"
            source = "curseforge"
            addon_id = "65387"
            directories = ["WeakAuras", "WeakAurasOptions"]
            is_dependency = false
            required_by = []
            installed_file_id = 5678901
            display_name = "WeakAuras 5.12.8"
            channel = "beta"
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.installed_file_id, Some(5678901));
        assert_eq!(addon.display_name, Some("WeakAuras 5.12.8".to_string()));
        assert_eq!(addon.channel, Some(ReleaseChannel::Beta));
        assert_eq!(addon.ignored, None);
    }

    #[test]
    fn installed_addon_ignored_flag() {
        let toml_str = r#"
            name = "Plumber"
            slug = "plumber"
            version = "1.8.8"
            source = "curseforge"
            addon_id = "12345"
            directories = ["Plumber"]
            is_dependency = false
            required_by = []
            ignored = true
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.ignored, Some(true));
        assert!(addon.is_ignored());
    }

    #[test]
    fn installed_addon_ignored_false() {
        let toml_str = r#"
            name = "Plumber"
            slug = "plumber"
            version = "1.8.8"
            source = "curseforge"
            addon_id = "12345"
            directories = ["Plumber"]
            is_dependency = false
            required_by = []
            ignored = false
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.ignored, Some(false));
        assert!(!addon.is_ignored());
    }

    #[test]
    fn installed_addon_with_game_versions() {
        let toml_str = r#"
            name = "Details"
            slug = "details"
            version = "1.2.3"
            source = "curseforge"
            addon_id = "99999"
            directories = ["Details", "Details_DataStorage"]
            is_dependency = false
            required_by = []
            installed_file_id = 1234567
            game_versions = ["11.1.0", "Retail"]
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(
            addon.game_versions,
            Some(vec!["11.1.0".to_string(), "Retail".to_string()])
        );
    }

    #[test]
    fn installed_addon_with_released_at() {
        let toml_str = r#"
            name = "WeakAuras"
            slug = "weakauras-2"
            version = "5.12.8"
            source = "curseforge"
            addon_id = "65387"
            directories = ["WeakAuras", "WeakAurasOptions"]
            is_dependency = false
            required_by = []
            installed_file_id = 5678901
            released_at = "2025-02-15T10:30:00Z"
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.released_at, Some("2025-02-15T10:30:00Z".to_string()));
    }

    #[test]
    fn installed_addon_auto_update_flag() {
        let toml_str = r#"
            name = "Details"
            slug = "details"
            version = "1.2.3"
            source = "curseforge"
            addon_id = "99999"
            directories = ["Details"]
            is_dependency = false
            required_by = []
            auto_update = true
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.auto_update, Some(true));
        assert!(addon.is_auto_update());
    }

    #[test]
    fn installed_addon_auto_update_false() {
        let toml_str = r#"
            name = "Details"
            slug = "details"
            version = "1.2.3"
            source = "curseforge"
            addon_id = "99999"
            directories = ["Details"]
            is_dependency = false
            required_by = []
            auto_update = false
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.auto_update, Some(false));
        assert!(!addon.is_auto_update());
    }

    #[test]
    fn release_channel_serde_roundtrip() {
        assert_eq!(
            serde_json::to_string(&ReleaseChannel::Stable).unwrap(),
            "\"stable\""
        );
        assert_eq!(
            serde_json::to_string(&ReleaseChannel::Beta).unwrap(),
            "\"beta\""
        );
        assert_eq!(
            serde_json::to_string(&ReleaseChannel::Alpha).unwrap(),
            "\"alpha\""
        );
        let parsed: ReleaseChannel = serde_json::from_str("\"stable\"").unwrap();
        assert_eq!(parsed, ReleaseChannel::Stable);
    }

    #[test]
    fn release_channel_includes_release_type() {
        assert!(ReleaseChannel::Stable.includes_release_type(1));
        assert!(!ReleaseChannel::Stable.includes_release_type(2));
        assert!(!ReleaseChannel::Stable.includes_release_type(3));

        assert!(ReleaseChannel::Beta.includes_release_type(1));
        assert!(ReleaseChannel::Beta.includes_release_type(2));
        assert!(!ReleaseChannel::Beta.includes_release_type(3));

        assert!(ReleaseChannel::Alpha.includes_release_type(1));
        assert!(ReleaseChannel::Alpha.includes_release_type(2));
        assert!(ReleaseChannel::Alpha.includes_release_type(3));
    }

    #[test]
    fn release_channel_ordering() {
        assert!(ReleaseChannel::Stable < ReleaseChannel::Beta);
        assert!(ReleaseChannel::Beta < ReleaseChannel::Alpha);
    }

    #[test]
    fn release_channel_from_str() {
        assert_eq!(
            "stable".parse::<ReleaseChannel>().unwrap(),
            ReleaseChannel::Stable
        );
        assert_eq!(
            "release".parse::<ReleaseChannel>().unwrap(),
            ReleaseChannel::Stable
        );
        assert_eq!(
            "beta".parse::<ReleaseChannel>().unwrap(),
            ReleaseChannel::Beta
        );
        assert_eq!(
            "alpha".parse::<ReleaseChannel>().unwrap(),
            ReleaseChannel::Alpha
        );
        assert_eq!(
            "STABLE".parse::<ReleaseChannel>().unwrap(),
            ReleaseChannel::Stable
        );
        assert!("unknown".parse::<ReleaseChannel>().is_err());
    }

    #[test]
    fn release_channel_display() {
        assert_eq!(ReleaseChannel::Stable.to_string(), "stable");
        assert_eq!(ReleaseChannel::Beta.to_string(), "beta");
        assert_eq!(ReleaseChannel::Alpha.to_string(), "alpha");
    }

    #[test]
    fn search_result_total_pages() {
        let make = |total, page_size| SearchResult {
            addons: vec![],
            page: 1,
            page_size,
            total_count: total,
        };
        assert_eq!(make(0, 20).total_pages(), 0);
        assert_eq!(make(1, 20).total_pages(), 1);
        assert_eq!(make(20, 20).total_pages(), 1);
        assert_eq!(make(21, 20).total_pages(), 2);
        assert_eq!(make(100, 20).total_pages(), 5);
        assert_eq!(make(101, 20).total_pages(), 6);
        assert_eq!(make(10, 0).total_pages(), 0);
    }
}
