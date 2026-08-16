//! Addon source abstraction and implementations.
//!
//! The AddonSource trait defines the interface for addon sources (CurseForge, WoWInterface, etc.).
//! This allows wowctl to support multiple addon sources with a unified interface.

pub mod curseforge;

use crate::addon::{AddonInfo, ReleaseChannel, SearchResult, VersionInfo};
use crate::error::{Result, WowctlError};
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

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
}
