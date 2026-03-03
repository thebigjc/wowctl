//! Addon source abstraction and implementations.
//!
//! The AddonSource trait defines the interface for addon sources (CurseForge, WoWInterface, etc.).
//! This allows wowctl to support multiple addon sources with a unified interface.

pub mod curseforge;

use crate::addon::{AddonInfo, ReleaseChannel, SearchResult, VersionInfo};
use crate::error::Result;
use std::path::{Path, PathBuf};

/// Trait for addon sources. Implementations provide access to addon repositories.
pub trait AddonSource: Send + Sync {
    /// Searches for addons matching the query, with optional pagination (1-indexed page number).
    fn search(
        &self,
        query: &str,
        page: Option<u32>,
    ) -> impl std::future::Future<Output = Result<SearchResult>> + Send;

    /// Gets the latest version information for an addon, filtered by release channel.
    fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> impl std::future::Future<Output = Result<VersionInfo>> + Send;

    /// Downloads an addon file to the specified destination.
    fn download(
        &self,
        download_url: &str,
        destination: &Path,
    ) -> impl std::future::Future<Output = Result<PathBuf>> + Send;

    /// Resolves the list of required dependency IDs for an addon.
    fn resolve_dependencies(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> impl std::future::Future<Output = Result<Vec<String>>> + Send;

    /// Gets addon information by slug.
    fn get_addon_by_slug(
        &self,
        slug: &str,
    ) -> impl std::future::Future<Output = Result<AddonInfo>> + Send;
}
