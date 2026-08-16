//! Addon source abstraction and implementations.
//!
//! The AddonSource trait defines the interface for addon sources (CurseForge, WoWInterface, etc.).
//! This allows wowctl to support multiple addon sources with a unified interface.

pub mod curseforge;
pub mod wago;

use crate::addon::{AddonInfo, ReleaseChannel, SearchResult, VersionInfo};
use crate::config::Config;
use crate::error::{Result, WowctlError};
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::debug;

/// The addon platforms wowctl can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum SourceKind {
    #[value(name = "curseforge")]
    CurseForge,
    #[value(name = "wago")]
    Wago,
}

impl SourceKind {
    /// The canonical string stored in the registry's per-addon `source` field.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::CurseForge => "curseforge",
            SourceKind::Wago => "wago",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "curseforge" => Ok(Self::CurseForge),
            "wago" => Ok(Self::Wago),
            _ => Err(format!("unknown source '{s}' (expected: curseforge, wago)")),
        }
    }
}

/// Parses a user-supplied addon spec into (Source, Slug).
///
/// Accepted forms:
/// - `classcodex` — bare slug, defaults to CurseForge
/// - `wago:classcodex`, `curseforge:weakauras-2` — explicit source prefix
/// - `https://addons.wago.io/addons/classcodex` — Wago page URL
/// - `https://www.curseforge.com/wow/addons/weakauras-2` — CurseForge page URL
pub fn parse_addon_spec(input: &str) -> Result<(SourceKind, String)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(WowctlError::Source("Empty addon spec".to_string()));
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return parse_addon_url(input);
    }
    if let Some((prefix, rest)) = input.split_once(':') {
        let kind: SourceKind = prefix.parse().map_err(WowctlError::Source)?;
        if rest.is_empty() {
            return Err(WowctlError::Source(format!(
                "Missing slug after '{prefix}:'"
            )));
        }
        return Ok((kind, rest.to_string()));
    }
    Ok((SourceKind::CurseForge, input.to_string()))
}

/// Extracts the path segment immediately after `marker`, stopping at
/// `/`, `?`, or `#`. Returns None if the marker is absent or the segment empty.
fn slug_after<'a>(url: &'a str, marker: &str) -> Option<&'a str> {
    let idx = url.find(marker)? + marker.len();
    let slug = url[idx..].split(['/', '?', '#']).next().unwrap_or("");
    (!slug.is_empty()).then_some(slug)
}

fn parse_addon_url(url: &str) -> Result<(SourceKind, String)> {
    if let Some(slug) = slug_after(url, "addons.wago.io/addons/") {
        return Ok((SourceKind::Wago, slug.to_string()));
    }
    if let Some(slug) = slug_after(url, "curseforge.com/wow/addons/") {
        return Ok((SourceKind::CurseForge, slug.to_string()));
    }
    Err(WowctlError::Source(format!(
        "Unrecognized addon URL: {url} (expected a curseforge.com or addons.wago.io addon page URL)"
    )))
}

/// Lightweight version info from a batch mod lookup, sufficient for update detection.
#[derive(Debug)]
pub struct BatchVersionCheck {
    pub addon_id: String,
    pub file_id: Option<u32>,
    pub external_release_id: Option<String>,
    pub version: String,
    pub display_name: String,
    pub released_at: String,
}

impl BatchVersionCheck {
    /// Builds a batch-check entry from a full VersionInfo.
    pub fn from_version_info(addon_id: &str, v: &VersionInfo) -> Self {
        Self {
            addon_id: addon_id.to_string(),
            file_id: v.file_id,
            external_release_id: v.external_release_id.clone(),
            version: v.version.clone(),
            display_name: v.display_name.clone(),
            released_at: v.released_at.clone(),
        }
    }
}

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

    /// Gets addon information by its Source-assigned Addon ID.
    fn get_addon_info_by_id(
        &self,
        addon_id: &str,
    ) -> impl std::future::Future<Output = Result<AddonInfo>> + Send;

    /// Batch check of the latest version for many addons, keyed by Addon ID.
    /// Sources with a batch endpoint should override; the default loops the
    /// single-addon check and skips (with a debug log) addons that fail.
    fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> impl std::future::Future<Output = Result<HashMap<String, BatchVersionCheck>>> + Send
    {
        async move {
            let mut results = HashMap::new();
            for id in addon_ids {
                match self.get_latest_version(id, channel).await {
                    Ok(v) => {
                        results.insert(
                            id.to_string(),
                            BatchVersionCheck::from_version_info(id, &v),
                        );
                    }
                    Err(e) => {
                        tracing::debug!("Batch version check failed for {}: {}", id, e);
                    }
                }
            }
            Ok(results)
        }
    }

    /// Batch fetch of AddonInfo by Addon ID. Default loops get_addon_info_by_id.
    fn get_addon_infos_batch(
        &self,
        addon_ids: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<AddonInfo>>> + Send {
        async move {
            let mut infos = Vec::new();
            for id in addon_ids {
                infos.push(self.get_addon_info_by_id(id).await?);
            }
            Ok(infos)
        }
    }
}

/// Downloads a zip file via the given prepared request, validating that the
/// response is a real zip archive (not an HTML error page) before writing it
/// to `destination`. Shared by all Sources so download quality is identical.
pub(crate) async fn download_zip(
    request: reqwest::RequestBuilder,
    download_url: &str,
    destination: &Path,
) -> Result<PathBuf> {
    use crate::error::WowctlError;
    use tokio::io::AsyncWriteExt;

    debug!("Downloading from: {}", download_url);

    let response = request
        .send()
        .await
        .map_err(|e| WowctlError::Network(format!("Failed to download addon: {e}")))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("(not set)")
        .to_string();

    debug!("Response: status={}, content-type={}", status, content_type);

    if !status.is_success() {
        return Err(WowctlError::Network(format!(
            "Download failed with status: {status}"
        )));
    }

    // Reject HTML error pages that CDNs sometimes serve with 200 OK
    if content_type.contains("text/html") || content_type.contains("text/plain") {
        return Err(WowctlError::Network(format!(
            "Server returned {content_type} instead of a zip file — the download URL may be invalid: {download_url}"
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| WowctlError::Network(format!("Failed to read download: {e}")))?;

    debug!("Downloaded {} bytes", bytes.len());

    // Validate ZIP magic bytes (PK\x03\x04) before writing to disk
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        if bytes.len() < 1024 {
            debug!(
                "Response body for invalid zip (small, {} bytes): {:?}",
                bytes.len(),
                String::from_utf8_lossy(&bytes)
            );
        }
        return Err(WowctlError::Extraction(format!(
            "Downloaded file is not a valid zip archive (bad magic bytes). \
             Got {} bytes, first 4: {:02x?}. \
             The server may have returned an error page. URL: {}",
            bytes.len(),
            &bytes[..bytes.len().min(4)],
            download_url
        )));
    }

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut file = tokio::fs::File::create(destination).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    drop(file);

    debug!("Downloaded to: {}", destination.display());
    Ok(destination.to_path_buf())
}

/// A concrete Source behind enum dispatch. The AddonSource trait's async
/// methods (RPIT-in-trait) are not dyn-compatible, so commands hold this
/// enum instead of a trait object.
pub enum AnySource {
    CurseForge(curseforge::CurseForgeSource),
    Wago(wago::WagoSource),
}

impl fmt::Debug for AnySource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AnySource::CurseForge(_) => f.debug_tuple("CurseForge").field(&"<source>").finish(),
            AnySource::Wago(_) => f.debug_tuple("Wago").field(&"<source>").finish(),
        }
    }
}

impl AnySource {
    pub fn kind(&self) -> SourceKind {
        match self {
            AnySource::CurseForge(_) => SourceKind::CurseForge,
            AnySource::Wago(_) => SourceKind::Wago,
        }
    }
}

impl AddonSource for AnySource {
    async fn search(&self, query: &str, page: Option<u32>) -> Result<SearchResult> {
        match self {
            AnySource::CurseForge(s) => s.search(query, page).await,
            AnySource::Wago(s) => s.search(query, page).await,
        }
    }

    async fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<VersionInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_latest_version(addon_id, channel).await,
            AnySource::Wago(s) => s.get_latest_version(addon_id, channel).await,
        }
    }

    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        match self {
            AnySource::CurseForge(s) => s.download(download_url, destination).await,
            AnySource::Wago(s) => s.download(download_url, destination).await,
        }
    }

    async fn resolve_dependencies(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<Vec<String>> {
        match self {
            AnySource::CurseForge(s) => s.resolve_dependencies(addon_id, channel).await,
            AnySource::Wago(s) => s.resolve_dependencies(addon_id, channel).await,
        }
    }

    async fn get_addon_by_slug(&self, slug: &str) -> Result<AddonInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_by_slug(slug).await,
            AnySource::Wago(s) => s.get_addon_by_slug(slug).await,
        }
    }

    async fn get_addon_info_by_id(&self, addon_id: &str) -> Result<AddonInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_info_by_id(addon_id).await,
            AnySource::Wago(s) => s.get_addon_info_by_id(addon_id).await,
        }
    }

    // Explicit forwarding (not the trait default) so CurseForge's batch
    // endpoints keep being used.
    async fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> Result<HashMap<String, BatchVersionCheck>> {
        match self {
            AnySource::CurseForge(s) => s.get_latest_versions_batch(addon_ids, channel).await,
            AnySource::Wago(s) => s.get_latest_versions_batch(addon_ids, channel).await,
        }
    }

    async fn get_addon_infos_batch(&self, addon_ids: &[String]) -> Result<Vec<AddonInfo>> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_infos_batch(addon_ids).await,
            AnySource::Wago(s) => s.get_addon_infos_batch(addon_ids).await,
        }
    }
}

/// Constructs the client for a Source, resolving its credentials from config.
/// A missing Wago key is a MissingApiKey error. Callers handle that error in
/// one of three ways: propagate it as a guided hard error (e.g. `wowctl init`
/// with a Wago source selected); pre-check `config.get_wago_access_key().is_some()`
/// before calling, so the Err path is never hit (merged `search`); or call
/// this and catch the Err per group/source to degrade gracefully, skipping
/// just that source while others proceed (`update`).
pub fn build_source(kind: SourceKind, config: &Config) -> Result<AnySource> {
    match kind {
        SourceKind::CurseForge => {
            let api_key = config.get_api_key()?;
            Ok(AnySource::CurseForge(curseforge::CurseForgeSource::new(
                api_key,
            )?))
        }
        SourceKind::Wago => {
            let access_key = config.get_wago_access_key().ok_or_else(|| {
                WowctlError::MissingApiKey(
                    "Wago access key not found. Set WOWCTL_WAGO_ACCESS_KEY or run \
                     'wowctl config set wago_access_key <key>'. Personal access keys \
                     come from https://addons.wago.io/patreon and require the \
                     'Wago Addons Supporter' Patreon tier."
                        .to_string(),
                )
            })?;
            Ok(AnySource::Wago(wago::WagoSource::new(access_key)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_slug_defaults_to_curseforge() {
        assert_eq!(
            parse_addon_spec("weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn wago_prefix_selects_wago() {
        assert_eq!(
            parse_addon_spec("wago:classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn curseforge_prefix_selects_curseforge() {
        assert_eq!(
            parse_addon_spec("curseforge:weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn prefix_is_case_insensitive() {
        assert_eq!(
            parse_addon_spec("WAGO:classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_selects_wago() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_with_trailing_slash() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex/").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_with_query_string() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex?utm_source=x").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn curseforge_url_selects_curseforge() {
        assert_eq!(
            parse_addon_spec("https://www.curseforge.com/wow/addons/weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn curseforge_url_without_www() {
        assert_eq!(
            parse_addon_spec("https://curseforge.com/wow/addons/details").unwrap(),
            (SourceKind::CurseForge, "details".to_string())
        );
    }

    #[test]
    fn unknown_prefix_errors() {
        let err = parse_addon_spec("wowinterface:foo").unwrap_err();
        assert!(err.to_string().contains("unknown source"));
    }

    #[test]
    fn unknown_url_errors() {
        assert!(parse_addon_spec("https://example.com/addons/foo").is_err());
    }

    #[test]
    fn empty_slug_after_prefix_errors() {
        assert!(parse_addon_spec("wago:").is_err());
    }

    #[test]
    fn bare_wago_addons_url_root_errors() {
        assert!(parse_addon_spec("https://addons.wago.io/addons/").is_err());
    }

    #[test]
    fn empty_input_errors() {
        assert!(parse_addon_spec("").is_err());
        assert!(parse_addon_spec("   ").is_err());
    }

    #[test]
    fn source_kind_string_roundtrip() {
        assert_eq!(SourceKind::CurseForge.as_str(), "curseforge");
        assert_eq!(SourceKind::Wago.as_str(), "wago");
        assert_eq!(
            "curseforge".parse::<SourceKind>().unwrap(),
            SourceKind::CurseForge
        );
        assert_eq!("wago".parse::<SourceKind>().unwrap(), SourceKind::Wago);
        assert_eq!("WAGO".parse::<SourceKind>().unwrap(), SourceKind::Wago);
        assert!("wowinterface".parse::<SourceKind>().is_err());
        assert_eq!(SourceKind::Wago.to_string(), "wago");
        assert_eq!(SourceKind::CurseForge.to_string(), "curseforge");
    }

    /// The CLI-facing value names must match the strings persisted in the
    /// registry, so `--source` accepts exactly what `as_str` writes out.
    /// Without `#[value(name = ...)]`, clap would derive `curse-forge`.
    #[test]
    fn clap_value_names_match_canonical_strings() {
        use clap::ValueEnum;

        for kind in [SourceKind::CurseForge, SourceKind::Wago] {
            assert_eq!(kind.to_possible_value().unwrap().get_name(), kind.as_str());
        }
    }

    #[test]
    fn build_source_wago_without_key_errors_with_guidance() {
        // Force key absence regardless of the developer machine's env.
        // SAFETY: test-only env mutation; no other test reads this variable
        // concurrently via std::env in this crate's lib tests.
        unsafe { std::env::remove_var("WOWCTL_WAGO_ACCESS_KEY") };
        let config = crate::config::Config {
            wago_access_key: None,
            ..Default::default()
        };
        let err = build_source(SourceKind::Wago, &config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("addons.wago.io/patreon"), "got: {msg}");
        assert!(msg.contains("WOWCTL_WAGO_ACCESS_KEY"), "got: {msg}");
    }

    #[test]
    fn build_source_wago_with_key_succeeds() {
        let config = crate::config::Config {
            wago_access_key: Some("some-key".to_string()),
            ..Default::default()
        };
        let source = build_source(SourceKind::Wago, &config).unwrap();
        assert_eq!(source.kind(), SourceKind::Wago);
    }
}
