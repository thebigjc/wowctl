//! Wago Addons source implementation.
//!
//! Talks to the undocumented external API at addons.wago.io/api/external
//! using a personal access key (Bearer auth on every call, downloads
//! included). Reference implementation: WowUp's wago-addon-provider.ts.
//! See ADR-0001 for why this API and its constraints.

use crate::addon::{AddonInfo, ReleaseChannel, VersionInfo};
use crate::error::{Result, WowctlError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WagoSearchResponse {
    #[serde(default)]
    data: Vec<WagoSearchItem>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WagoSearchItem {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    display_name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    download_count: Option<f64>,
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    is_hidden_from_external: bool,
    #[serde(default)]
    releases: WagoReleases,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WagoAddonDetail {
    id: String,
    slug: String,
    display_name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    download_count: Option<f64>,
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    is_hidden_from_external: bool,
    #[serde(default)]
    recent_releases: WagoReleases,
}

/// Releases keyed by Wago stability tier. Tiers map 1:1 onto ReleaseChannel.
#[derive(Debug, Default, Deserialize)]
#[allow(dead_code)]
struct WagoReleases {
    #[serde(default)]
    stable: Option<WagoRelease>,
    #[serde(default)]
    beta: Option<WagoRelease>,
    #[serde(default)]
    alpha: Option<WagoRelease>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct WagoRelease {
    label: String,
    #[serde(default)]
    download_link: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    /// Wago's monotonically-increasing release marker; our external_release_id.
    #[serde(default)]
    logical_timestamp: Option<u64>,
    #[serde(default)]
    stability: Option<String>,
    #[serde(default)]
    supported_retail_patch: Option<String>,
}

#[derive(Debug, Serialize)]
#[allow(dead_code)]
struct WagoRecentsRequest<'a> {
    game_version: &'a str,
    addons: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WagoRecentsResponse {
    #[serde(default)]
    addons: HashMap<String, WagoRecentsEntry>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct WagoRecentsEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    recent_releases: WagoReleases,
}

/// Maps a ReleaseChannel to Wago's stability query-parameter value.
#[allow(dead_code)]
fn stability_param(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Beta => "beta",
        ReleaseChannel::Alpha => "alpha",
    }
}

/// Picks the newest release the channel allows (stable ⊆ beta ⊆ alpha),
/// newest judged by logical_timestamp.
#[allow(dead_code)]
fn select_release(releases: &WagoReleases, channel: ReleaseChannel) -> Option<&WagoRelease> {
    let mut candidates: Vec<&WagoRelease> = Vec::new();
    if let Some(r) = &releases.stable {
        candidates.push(r);
    }
    if channel >= ReleaseChannel::Beta
        && let Some(r) = &releases.beta
    {
        candidates.push(r);
    }
    if channel >= ReleaseChannel::Alpha
        && let Some(r) = &releases.alpha
    {
        candidates.push(r);
    }
    candidates
        .into_iter()
        .max_by_key(|r| r.logical_timestamp.unwrap_or(0))
}

/// Resolves an item's Slug: explicit field first, else the last path segment
/// of its website URL.
#[allow(dead_code)]
fn item_slug(item: &WagoSearchItem) -> Option<String> {
    if let Some(s) = &item.slug {
        return Some(s.clone());
    }
    item.website_url
        .as_deref()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[allow(dead_code)]
fn to_addon_info(item: &WagoSearchItem) -> AddonInfo {
    AddonInfo {
        id: item.id.clone(),
        name: item.display_name.clone(),
        slug: item_slug(item).unwrap_or_else(|| item.id.clone()),
        description: item.summary.clone(),
        download_count: item.download_count.map(|d| d as u64),
        source: "wago".to_string(),
    }
}

#[allow(dead_code)]
fn to_version_info(release: &WagoRelease, file_name: String) -> Result<VersionInfo> {
    let download_url = release.download_link.clone().ok_or_else(|| {
        WowctlError::Source(format!(
            "Wago release '{}' has no download link",
            release.label
        ))
    })?;
    Ok(VersionInfo {
        file_id: None,
        external_release_id: release.logical_timestamp.map(|t| t.to_string()),
        version: release.label.clone(),
        display_name: release.label.clone(),
        download_url,
        file_name,
        // Wago does not report file sizes; 0 makes the disk-space check a no-op.
        file_size: 0,
        game_versions: release
            .supported_retail_patch
            .clone()
            .map(|p| vec![p])
            .unwrap_or_default(),
        released_at: release.created_at.clone().unwrap_or_default(),
        dependencies: vec![],
        modules: vec![],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(label: &str, ts: u64) -> WagoRelease {
        WagoRelease {
            label: label.to_string(),
            download_link: Some(format!("https://example.com/{label}.zip")),
            created_at: Some("2026-08-01T00:00:00Z".to_string()),
            logical_timestamp: Some(ts),
            stability: None,
            supported_retail_patch: None,
        }
    }

    #[test]
    fn stable_channel_only_sees_stable() {
        let releases = WagoReleases {
            stable: Some(release("1.0", 100)),
            beta: Some(release("2.0-beta", 200)),
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Stable).unwrap();
        assert_eq!(picked.label, "1.0");
    }

    #[test]
    fn beta_channel_picks_newest_of_stable_and_beta() {
        let releases = WagoReleases {
            stable: Some(release("1.0", 100)),
            beta: Some(release("2.0-beta", 200)),
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Beta).unwrap();
        assert_eq!(picked.label, "2.0-beta");
    }

    #[test]
    fn beta_channel_prefers_newer_stable() {
        let releases = WagoReleases {
            stable: Some(release("2.1", 400)),
            beta: Some(release("2.0-beta", 200)),
            alpha: None,
        };
        let picked = select_release(&releases, ReleaseChannel::Beta).unwrap();
        assert_eq!(picked.label, "2.1");
    }

    #[test]
    fn alpha_channel_sees_all_tiers() {
        let releases = WagoReleases {
            stable: None,
            beta: None,
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Alpha).unwrap();
        assert_eq!(picked.label, "3.0-alpha");
    }

    #[test]
    fn no_eligible_release_returns_none() {
        let releases = WagoReleases {
            stable: None,
            beta: Some(release("2.0-beta", 200)),
            alpha: None,
        };
        assert!(select_release(&releases, ReleaseChannel::Stable).is_none());
    }

    #[test]
    fn stability_param_maps_channels() {
        assert_eq!(stability_param(ReleaseChannel::Stable), "stable");
        assert_eq!(stability_param(ReleaseChannel::Beta), "beta");
        assert_eq!(stability_param(ReleaseChannel::Alpha), "alpha");
    }

    #[test]
    fn item_slug_prefers_slug_field() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: Some("classcodex".to_string()),
            display_name: "ClassCodex".to_string(),
            summary: None,
            download_count: None,
            website_url: Some("https://addons.wago.io/addons/other".to_string()),
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        assert_eq!(item_slug(&item), Some("classcodex".to_string()));
    }

    #[test]
    fn item_slug_falls_back_to_website_url() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: None,
            display_name: "ClassCodex".to_string(),
            summary: None,
            download_count: None,
            website_url: Some("https://addons.wago.io/addons/classcodex/".to_string()),
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        assert_eq!(item_slug(&item), Some("classcodex".to_string()));
    }

    #[test]
    fn to_addon_info_maps_fields_and_tags_source() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: Some("classcodex".to_string()),
            display_name: "ClassCodex".to_string(),
            summary: Some("Class guide addon".to_string()),
            download_count: Some(1234.0),
            website_url: None,
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        let info = to_addon_info(&item);
        assert_eq!(info.id, "rNkynwKa");
        assert_eq!(info.slug, "classcodex");
        assert_eq!(info.name, "ClassCodex");
        assert_eq!(info.description, Some("Class guide addon".to_string()));
        assert_eq!(info.download_count, Some(1234));
        assert_eq!(info.source, "wago");
    }

    #[test]
    fn to_version_info_maps_release() {
        let mut r = release("1.2.0", 1755100000000000);
        r.supported_retail_patch = Some("11.2.0".to_string());
        let v = to_version_info(&r, "classcodex.zip".to_string()).unwrap();
        assert_eq!(v.file_id, None);
        assert_eq!(v.external_release_id, Some("1755100000000000".to_string()));
        assert_eq!(v.version, "1.2.0");
        assert_eq!(v.display_name, "1.2.0");
        assert_eq!(v.download_url, "https://example.com/1.2.0.zip");
        assert_eq!(v.file_name, "classcodex.zip");
        assert_eq!(v.game_versions, vec!["11.2.0".to_string()]);
        assert_eq!(v.released_at, "2026-08-01T00:00:00Z");
        assert!(v.dependencies.is_empty());
        assert!(v.modules.is_empty());
    }

    #[test]
    fn to_version_info_without_download_link_errors() {
        let mut r = release("1.2.0", 1);
        r.download_link = None;
        assert!(to_version_info(&r, "x.zip".to_string()).is_err());
    }

    #[test]
    fn search_response_deserializes() {
        let json = serde_json::json!({
            "data": [{
                "id": "rNkynwKa",
                "slug": "classcodex",
                "display_name": "ClassCodex",
                "summary": "Class guide addon",
                "download_count": 1234,
                "website_url": "https://addons.wago.io/addons/classcodex",
                "releases": {
                    "stable": {
                        "label": "1.2.0",
                        "download_link": "https://addons.wago.io/api/external/files/abc/download",
                        "created_at": "2026-08-01T00:00:00Z",
                        "logical_timestamp": 1755100000000000u64
                    }
                }
            }]
        });
        let resp: WagoSearchResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].id, "rNkynwKa");
        assert!(!resp.data[0].is_hidden_from_external);
    }

    #[test]
    fn detail_deserializes_with_recent_releases() {
        let json = serde_json::json!({
            "id": "rNkynwKa",
            "slug": "classcodex",
            "display_name": "ClassCodex",
            "summary": "Class guide addon",
            "download_count": 1234,
            "website_url": "https://addons.wago.io/addons/classcodex",
            "is_hidden_from_external": false,
            "recent_releases": {
                "stable": {
                    "label": "1.2.0",
                    "download_link": "https://addons.wago.io/api/external/files/abc/download",
                    "created_at": "2026-08-01T00:00:00Z",
                    "logical_timestamp": 1755100000000000u64,
                    "stability": "stable",
                    "supported_retail_patch": "11.2.0"
                }
            }
        });
        let detail: WagoAddonDetail = serde_json::from_value(json).unwrap();
        assert_eq!(detail.slug, "classcodex");
        assert_eq!(
            detail.recent_releases.stable.as_ref().unwrap().label,
            "1.2.0"
        );
    }

    #[test]
    fn recents_response_deserializes() {
        let json = serde_json::json!({
            "addons": {
                "rNkynwKa": {
                    "id": "rNkynwKa",
                    "recent_releases": {
                        "stable": {
                            "label": "1.3.0",
                            "download_link": "https://example.com/1.3.0.zip",
                            "created_at": "2026-08-10T00:00:00Z",
                            "logical_timestamp": 1755200000000000u64
                        }
                    }
                }
            }
        });
        let resp: WagoRecentsResponse = serde_json::from_value(json).unwrap();
        let entry = resp.addons.get("rNkynwKa").unwrap();
        assert_eq!(
            entry.recent_releases.stable.as_ref().unwrap().label,
            "1.3.0"
        );
    }

    #[test]
    fn recents_request_serializes() {
        let req = WagoRecentsRequest {
            game_version: "retail",
            addons: vec!["rNkynwKa".to_string(), "abc123".to_string()],
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({"game_version": "retail", "addons": ["rNkynwKa", "abc123"]})
        );
    }

    #[test]
    fn hidden_flag_deserializes_true() {
        let json = serde_json::json!({
            "id": "x", "display_name": "Hidden", "is_hidden_from_external": true
        });
        let item: WagoSearchItem = serde_json::from_value(json).unwrap();
        assert!(item.is_hidden_from_external);
    }
}
