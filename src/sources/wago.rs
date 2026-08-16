//! Wago Addons source implementation.
//!
//! Talks to the undocumented external API at addons.wago.io/api/external
//! using a personal access key (Bearer auth on every call, downloads
//! included). Reference implementation: WowUp's wago-addon-provider.ts.
//! See ADR-0001 for why this API and its constraints.

use crate::addon::{AddonInfo, ReleaseChannel, SearchResult, VersionInfo};
use crate::circuit_breaker::CircuitBreaker;
use crate::error::{Result, WowctlError};
use crate::sources::{AddonSource, BatchVersionCheck};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing::{debug, warn};

#[derive(Debug, Deserialize)]
struct WagoSearchResponse {
    #[serde(default)]
    data: Vec<WagoSearchItem>,
}

#[derive(Debug, Deserialize)]
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
    /// Live API omits this field entirely (hidden addons are filtered
    /// server-side); kept for defensive client-side filtering if it ever
    /// reappears.
    #[serde(default)]
    is_hidden_from_external: bool,
    /// Wire-format documentation field; search results don't carry the
    /// release we'd download (that comes from the detail endpoint).
    #[serde(default)]
    #[allow(dead_code)]
    releases: WagoReleases,
}

#[derive(Debug, Deserialize)]
struct WagoAddonDetail {
    id: String,
    slug: String,
    display_name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    download_count: Option<f64>,
    /// Wire-format documentation field; detail responses already carry
    /// `slug` directly, so we never need the website URL fallback here.
    #[serde(default)]
    #[allow(dead_code)]
    website_url: Option<String>,
    /// Live API omits this field entirely (hidden addons are filtered
    /// server-side); kept for defensive client-side filtering if it ever
    /// reappears.
    #[serde(default)]
    is_hidden_from_external: bool,
    #[serde(rename = "recent_release", default)]
    recent_releases: WagoReleases,
}

/// Releases keyed by Wago stability tier. Tiers map 1:1 onto ReleaseChannel.
#[derive(Debug, Default, Deserialize)]
struct WagoReleases {
    #[serde(default)]
    stable: Option<WagoRelease>,
    #[serde(default)]
    beta: Option<WagoRelease>,
    #[serde(default)]
    alpha: Option<WagoRelease>,
}

#[derive(Debug, Clone, Deserialize)]
struct WagoRelease {
    label: String,
    /// Detail/search responses use `download_link`; `_recents` batch
    /// responses use `link` for the same thing.
    #[serde(alias = "link", default)]
    download_link: Option<String>,
    /// Byte-identical across detail/recents/search for the same release;
    /// our release-identity field (`external_release_id`). No numeric
    /// `logical_timestamp` or `id` on detail/recents.
    #[serde(default)]
    created_at: Option<String>,
    /// Wire-format documentation field; we don't branch on it (releases are
    /// already keyed by stability tier).
    #[serde(default)]
    #[allow(dead_code)]
    stability: Option<String>,
    /// Detail/search responses use `supported_retail_patch`; `_recents`
    /// batch responses use `patch` for the same thing.
    #[serde(alias = "patch", default)]
    supported_retail_patch: Option<String>,
}

#[derive(Debug, Serialize)]
struct WagoRecentsRequest<'a> {
    game_version: &'a str,
    addons: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WagoRecentsResponse {
    #[serde(default)]
    addons: HashMap<String, WagoRecentsEntry>,
}

#[derive(Debug, Deserialize)]
struct WagoRecentsEntry {
    /// Wire-format documentation field; the response is already keyed by
    /// addon ID in the surrounding map.
    #[serde(default)]
    #[allow(dead_code)]
    id: Option<String>,
    #[serde(default)]
    recent_releases: WagoReleases,
}

/// Maps a ReleaseChannel to Wago's stability query-parameter value.
fn stability_param(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Beta => "beta",
        ReleaseChannel::Alpha => "alpha",
    }
}

/// Picks the newest release the channel allows (stable ⊆ beta ⊆ alpha),
/// newest judged by created_at (fixed-width ISO-8601 UTC, so lexicographic
/// order equals chronological order).
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
        .max_by_key(|r| r.created_at.as_deref().unwrap_or(""))
}

/// Resolves an item's Slug: explicit field first, else the last path segment
/// of its website URL.
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

fn to_version_info(release: &WagoRelease, file_name: String) -> Result<VersionInfo> {
    let download_url = release.download_link.clone().ok_or_else(|| {
        WowctlError::Source(format!(
            "Wago release '{}' has no download link",
            release.label
        ))
    })?;
    Ok(VersionInfo {
        file_id: None,
        external_release_id: release.created_at.clone(),
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

const WAGO_API_BASE: &str = "https://addons.wago.io/api/external";
const GAME_VERSION_RETAIL: &str = "retail";
const HTTP_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Wago Addons source implementation. Every call carries Bearer auth.
pub struct WagoSource {
    client: Client,
    access_key: String,
    api_base: String,
    circuit_breaker: CircuitBreaker,
}

/// The 401 error users see; points at where keys come from (ADR-0001).
fn unauthorized_error() -> WowctlError {
    WowctlError::Unauthorized(
        "Wago rejected the access key (HTTP 401). Personal access keys come from \
         https://addons.wago.io/patreon and require the 'Wago Addons Supporter' \
         Patreon tier ($3/month). Update WOWCTL_WAGO_ACCESS_KEY or run \
         'wowctl config set wago_access_key <key>'."
            .to_string(),
    )
}

impl WagoSource {
    /// Creates a new Wago source with the provided personal access key.
    pub fn new(access_key: String) -> Result<Self> {
        Self::with_base_url(access_key, WAGO_API_BASE.to_string())
    }

    /// Creates a Wago source pointed at a custom API base URL (tests).
    pub fn with_base_url(access_key: String, api_base: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent(format!("wowctl/{}", env!("WOWCTL_VERSION")))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| WowctlError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            access_key,
            api_base,
            circuit_breaker: CircuitBreaker::new(),
        })
    }

    /// Records a request outcome with the circuit breaker. 404s and auth
    /// failures are the caller's problem, not an API outage.
    fn record_circuit_breaker_result<T>(&self, result: &Result<T>) {
        match result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(WowctlError::AddonNotFound(_))
            | Err(WowctlError::Unauthorized(_))
            | Err(WowctlError::CircuitBreakerOpen) => {}
            Err(_) => self.circuit_breaker.record_failure(),
        }
    }

    /// Executes a request built by `build` with retry/backoff, mapping Wago's
    /// status codes onto wowctl errors. Responses deserialize bare (no `data`
    /// wrapper except where the model itself declares one).
    async fn execute_with_retry<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T> {
        if !self.circuit_breaker.allow_request() {
            return Err(WowctlError::CircuitBreakerOpen);
        }
        let result = self.execute_with_retry_inner(build).await;
        self.record_circuit_breaker_result(&result);
        result
    }

    async fn execute_with_retry_inner<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T> {
        let mut attempts = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            attempts += 1;
            match build()
                .bearer_auth(&self.access_key)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    debug!("Wago response status: {}", status);

                    if status.is_success() {
                        return response.json::<T>().await.map_err(|e| {
                            WowctlError::Source(format!("Failed to parse Wago API response: {e}"))
                        });
                    } else if status.as_u16() == 401 {
                        return Err(unauthorized_error());
                    } else if status.as_u16() == 404 {
                        return Err(WowctlError::AddonNotFound(
                            "Addon not found on Wago".to_string(),
                        ));
                    } else if status.as_u16() == 429 {
                        if attempts >= MAX_RETRIES {
                            return Err(WowctlError::Network(
                                "Rate limited by Wago API after multiple retries".to_string(),
                            ));
                        }
                        warn!("Rate limited by Wago API, retrying with backoff...");
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(WowctlError::Source(format!(
                            "Wago API error ({status}): {error_text}"
                        )));
                    }
                }
                Err(e) => {
                    warn!("Network error: {}", e);
                    if attempts >= MAX_RETRIES {
                        return Err(WowctlError::Network(format!(
                            "Failed to connect to Wago API after {MAX_RETRIES} attempts: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms *= 2;
        }
    }

    /// Fetches addon detail by Wago Addon ID (or slug — the endpoint accepts
    /// both), enforcing is_hidden_from_external.
    async fn get_detail(&self, id_or_slug: &str) -> Result<WagoAddonDetail> {
        let url = format!("{}/addons/{}", self.api_base, id_or_slug);
        let detail: WagoAddonDetail = self
            .execute_with_retry(|| {
                self.client
                    .get(&url)
                    .query(&[("game_version", GAME_VERSION_RETAIL)])
            })
            .await?;
        if detail.is_hidden_from_external {
            return Err(WowctlError::AddonNotFound(format!(
                "Addon '{}' is hidden from external clients by its author",
                detail.slug
            )));
        }
        Ok(detail)
    }

    async fn search_items(&self, query: &str) -> Result<Vec<WagoSearchItem>> {
        let url = format!("{}/addons/_search", self.api_base);
        let response: WagoSearchResponse = self
            .execute_with_retry(|| {
                self.client.get(&url).query(&[
                    ("query", query),
                    ("game_version", GAME_VERSION_RETAIL),
                    ("stability", stability_param(ReleaseChannel::Stable)),
                ])
            })
            .await?;
        Ok(response
            .data
            .into_iter()
            .filter(|item| !item.is_hidden_from_external)
            .collect())
    }
}

fn detail_to_addon_info(detail: &WagoAddonDetail) -> AddonInfo {
    AddonInfo {
        id: detail.id.clone(),
        name: detail.display_name.clone(),
        slug: detail.slug.clone(),
        description: detail.summary.clone(),
        download_count: detail.download_count.map(|d| d as u64),
        source: "wago".to_string(),
    }
}

impl AddonSource for WagoSource {
    async fn search(&self, query: &str, _page: Option<u32>) -> Result<SearchResult> {
        debug!("Searching Wago for: {}", query);
        let items = self.search_items(query).await?;
        let addons: Vec<AddonInfo> = items.iter().map(to_addon_info).collect();
        let count = addons.len() as u32;
        // Wago's search pagination is undocumented; treat results as one page.
        Ok(SearchResult {
            addons,
            page: 1,
            page_size: count,
            total_count: count,
        })
    }

    async fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<VersionInfo> {
        debug!(
            "Getting latest Wago version for {} (channel: {})",
            addon_id, channel
        );
        let detail = self.get_detail(addon_id).await?;
        let release = select_release(&detail.recent_releases, channel).ok_or_else(|| {
            WowctlError::Source(format!(
                "No {channel} release found on Wago for '{}'",
                detail.slug
            ))
        })?;
        to_version_info(release, format!("{}.zip", detail.slug))
    }

    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        // Wago download links are signed and expiring; the Bearer token must
        // accompany the request (ADR-0001).
        crate::sources::download_zip(
            self.client.get(download_url).bearer_auth(&self.access_key),
            download_url,
            destination,
        )
        .await
    }

    async fn resolve_dependencies(
        &self,
        _addon_id: &str,
        _channel: ReleaseChannel,
    ) -> Result<Vec<String>> {
        // The Wago API exposes no dependency data we rely on (issue #8).
        Ok(vec![])
    }

    async fn get_addon_by_slug(&self, slug: &str) -> Result<AddonInfo> {
        debug!("Looking up Wago addon by slug: {}", slug);
        let items = self.search_items(slug).await?;
        if let Some(item) = items.iter().find(|i| item_slug(i).as_deref() == Some(slug)) {
            return Ok(to_addon_info(item));
        }
        // Fallback: the detail endpoint also resolves slugs directly.
        match self.get_detail(slug).await {
            Ok(detail) if detail.slug == slug => Ok(detail_to_addon_info(&detail)),
            Ok(_) | Err(WowctlError::AddonNotFound(_)) => Err(WowctlError::AddonNotFound(
                format!("Addon '{slug}' not found on Wago"),
            )),
            Err(e) => Err(e),
        }
    }

    async fn get_addon_info_by_id(&self, addon_id: &str) -> Result<AddonInfo> {
        let detail = self.get_detail(addon_id).await?;
        Ok(detail_to_addon_info(&detail))
    }

    async fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> Result<HashMap<String, BatchVersionCheck>> {
        if addon_ids.is_empty() {
            return Ok(HashMap::new());
        }
        debug!("Batch checking {} Wago addon(s)", addon_ids.len());
        let url = format!("{}/addons/_recents", self.api_base);
        let body = WagoRecentsRequest {
            game_version: GAME_VERSION_RETAIL,
            addons: addon_ids.iter().map(|s| s.to_string()).collect(),
        };
        let response: WagoRecentsResponse = self
            .execute_with_retry(|| self.client.post(&url).json(&body))
            .await?;

        let mut results = HashMap::new();
        for (id, entry) in &response.addons {
            if let Some(release) = select_release(&entry.recent_releases, channel) {
                results.insert(
                    id.clone(),
                    BatchVersionCheck {
                        addon_id: id.clone(),
                        file_id: None,
                        external_release_id: release.created_at.clone(),
                        version: release.label.clone(),
                        display_name: release.label.clone(),
                        released_at: release.created_at.clone().unwrap_or_default(),
                    },
                );
            }
        }
        debug!(
            "Wago batch check returned {} of {} addon(s)",
            results.len(),
            addon_ids.len()
        );
        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(label: &str, created_at: &str) -> WagoRelease {
        WagoRelease {
            label: label.to_string(),
            download_link: Some(format!("https://example.com/{label}.zip")),
            created_at: Some(created_at.to_string()),
            stability: None,
            supported_retail_patch: None,
        }
    }

    #[test]
    fn stable_channel_only_sees_stable() {
        let releases = WagoReleases {
            stable: Some(release("1.0", "2026-08-01T00:00:00.000000Z")),
            beta: Some(release("2.0-beta", "2026-08-02T00:00:00.000000Z")),
            alpha: Some(release("3.0-alpha", "2026-08-03T00:00:00.000000Z")),
        };
        let picked = select_release(&releases, ReleaseChannel::Stable).unwrap();
        assert_eq!(picked.label, "1.0");
    }

    #[test]
    fn beta_channel_picks_newest_of_stable_and_beta() {
        let releases = WagoReleases {
            stable: Some(release("1.0", "2026-08-01T00:00:00.000000Z")),
            beta: Some(release("2.0-beta", "2026-08-02T00:00:00.000000Z")),
            alpha: Some(release("3.0-alpha", "2026-08-03T00:00:00.000000Z")),
        };
        let picked = select_release(&releases, ReleaseChannel::Beta).unwrap();
        assert_eq!(picked.label, "2.0-beta");
    }

    #[test]
    fn beta_channel_prefers_newer_stable() {
        let releases = WagoReleases {
            stable: Some(release("2.1", "2026-08-04T00:00:00.000000Z")),
            beta: Some(release("2.0-beta", "2026-08-02T00:00:00.000000Z")),
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
            alpha: Some(release("3.0-alpha", "2026-08-03T00:00:00.000000Z")),
        };
        let picked = select_release(&releases, ReleaseChannel::Alpha).unwrap();
        assert_eq!(picked.label, "3.0-alpha");
    }

    #[test]
    fn no_eligible_release_returns_none() {
        let releases = WagoReleases {
            stable: None,
            beta: Some(release("2.0-beta", "2026-08-02T00:00:00.000000Z")),
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
        let mut r = release("1.2.0", "2026-08-06T13:42:41.000000Z");
        r.supported_retail_patch = Some("11.2.0".to_string());
        let v = to_version_info(&r, "classcodex.zip".to_string()).unwrap();
        assert_eq!(v.file_id, None);
        assert_eq!(
            v.external_release_id,
            Some("2026-08-06T13:42:41.000000Z".to_string())
        );
        assert_eq!(v.version, "1.2.0");
        assert_eq!(v.display_name, "1.2.0");
        assert_eq!(v.download_url, "https://example.com/1.2.0.zip");
        assert_eq!(v.file_name, "classcodex.zip");
        assert_eq!(v.game_versions, vec!["11.2.0".to_string()]);
        assert_eq!(v.released_at, "2026-08-06T13:42:41.000000Z");
        assert!(v.dependencies.is_empty());
        assert!(v.modules.is_empty());
    }

    #[test]
    fn to_version_info_without_download_link_errors() {
        let mut r = release("1.2.0", "2026-08-01T00:00:00.000000Z");
        r.download_link = None;
        assert!(to_version_info(&r, "x.zip".to_string()).is_err());
    }

    #[test]
    fn search_response_deserializes() {
        // Real search items carry no `slug` field; `website_url` is the
        // fallback (see item_slug). Search release tiers DO carry
        // logical_timestamp on the wire; the model no longer reads it.
        let json = serde_json::json!({
            "data": [{
                "id": "rNkynwKa",
                "display_name": "ClassCodex",
                "summary": "Class guide addon",
                "download_count": 1234,
                "website_url": "https://addons.wago.io/addons/classcodex",
                "releases": {
                    "stable": {
                        "label": "1.2.0",
                        "download_link": "https://addons.wago.io/api/external/files/abc/download",
                        "created_at": "2026-08-06T13:42:41.000000Z",
                        "logical_timestamp": 1755100000000000u64
                    }
                }
            }]
        });
        let resp: WagoSearchResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].id, "rNkynwKa");
        assert_eq!(resp.data[0].slug, None);
        assert!(!resp.data[0].is_hidden_from_external);
    }

    #[test]
    fn detail_deserializes_with_recent_release() {
        // Real detail responses key the releases object as `recent_release`
        // (singular) and have no `is_hidden_from_external`, `id`, or
        // `logical_timestamp` on the release.
        let json = serde_json::json!({
            "id": "rNkynwKa",
            "slug": "classcodex",
            "display_name": "ClassCodex",
            "summary": "Class guide addon",
            "download_count": 1234,
            "website_url": "https://addons.wago.io/addons/classcodex",
            "recent_release": {
                "stable": {
                    "label": "1.2.0",
                    "download_link": "https://addons.wago.io/api/external/files/abc/download",
                    "created_at": "2026-08-06T13:42:41.000000Z",
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
        // Real `_recents` release tiers use `link`/`patch`/`id`, no
        // `logical_timestamp`.
        let json = serde_json::json!({
            "addons": {
                "rNkynwKa": {
                    "id": "rNkynwKa",
                    "recent_releases": {
                        "stable": {
                            "label": "1.3.0",
                            "link": "https://example.com/1.3.0.zip",
                            "created_at": "2026-08-10T00:00:00.000000Z",
                            "id": "some-release-id",
                            "patch": "11.2.0"
                        }
                    }
                }
            }
        });
        let resp: WagoRecentsResponse = serde_json::from_value(json).unwrap();
        let entry = resp.addons.get("rNkynwKa").unwrap();
        let release = entry.recent_releases.stable.as_ref().unwrap();
        assert_eq!(release.label, "1.3.0");
        assert_eq!(
            release.download_link,
            Some("https://example.com/1.3.0.zip".to_string())
        );
        assert_eq!(release.supported_retail_patch, Some("11.2.0".to_string()));
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
