//! CurseForge addon source implementation.
//!
//! Provides integration with the CurseForge API for searching, downloading,
//! and managing World of Warcraft addons.

use crate::addon::{
    AddonInfo, DependencyInfo, DependencyType, ReleaseChannel, SearchResult, VersionInfo,
};
use crate::circuit_breaker::CircuitBreaker;
use crate::error::{Result, WowctlError};
use crate::sources::AddonSource;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tracing::{debug, info, warn};

const CURSEFORGE_API_BASE: &str = "https://api.curseforge.com/v1";
const CURSEFORGE_CDN_BASE: &str = "https://edge.forgecdn.net/files";
const WOW_GAME_ID: u32 = 1;
const WOW_ADDONS_CLASS_ID: u32 = 1;
const WOW_RETAIL_VERSION_TYPE_ID: u32 = 517;
const HTTP_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Constructs a CurseForge CDN download URL from a file ID and filename.
///
/// The CDN path format is `/files/{id / 1000}/{id % 1000}/{filename}`,
/// matching the URLs the API returns when `downloadUrl` is populated.
fn build_cdn_url(file_id: u32, file_name: &str) -> String {
    let part1 = file_id / 1000;
    let part2 = file_id % 1000;
    format!("{CURSEFORGE_CDN_BASE}/{part1}/{part2}/{file_name}")
}

/// CurseForge addon source implementation.
pub struct CurseForgeSource {
    client: Client,
    api_key: String,
    circuit_breaker: CircuitBreaker,
}

#[derive(Debug, Deserialize)]
struct ApiResponse<T> {
    data: T,
}

#[derive(Debug, Deserialize)]
struct PaginatedApiResponse<T> {
    data: T,
    pagination: CfPagination,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct CfPagination {
    index: u32,
    #[serde(rename = "pageSize")]
    page_size: u32,
    #[serde(rename = "resultCount")]
    result_count: u32,
    #[serde(rename = "totalCount")]
    total_count: u32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfMod {
    id: u32,
    name: String,
    slug: String,
    summary: Option<String>,
    #[serde(rename = "downloadCount")]
    download_count: Option<f64>,
    #[serde(rename = "latestFiles")]
    latest_files: Option<Vec<CfFile>>,
    links: Option<CfLinks>,
    #[serde(rename = "allowModDistribution")]
    allow_mod_distribution: Option<bool>,
    #[serde(rename = "latestFilesIndexes", default)]
    latest_files_indexes: Vec<CfLatestFileIndex>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfLinks {
    #[serde(rename = "websiteUrl")]
    website_url: Option<String>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfFile {
    id: u32,
    #[serde(rename = "displayName")]
    display_name: String,
    #[serde(rename = "fileName")]
    file_name: String,
    #[serde(rename = "downloadUrl")]
    download_url: Option<String>,
    #[serde(rename = "fileLength")]
    file_length: u64,
    #[serde(rename = "gameVersions")]
    game_versions: Vec<String>,
    dependencies: Vec<CfDependency>,
    #[serde(rename = "fileDate")]
    file_date: String,
    #[serde(rename = "releaseType", default = "default_release_type")]
    release_type: u32,
    #[serde(rename = "fileFingerprint", default)]
    file_fingerprint: u64,
    #[serde(default)]
    modules: Vec<CfModule>,
}

fn default_release_type() -> u32 {
    1
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfModule {
    name: String,
    #[serde(default)]
    fingerprint: u64,
}

#[derive(Debug, Deserialize)]
struct CfDependency {
    #[serde(rename = "modId")]
    mod_id: u32,
    #[serde(rename = "relationType")]
    relation_type: u32,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfLatestFileIndex {
    #[serde(rename = "fileId")]
    file_id: u32,
    #[serde(rename = "gameVersionTypeId")]
    game_version_type_id: Option<u32>,
    #[serde(rename = "releaseType", default = "default_release_type")]
    release_type: u32,
}

#[derive(Debug, Serialize)]
struct SearchParams {
    #[serde(rename = "gameId")]
    game_id: u32,
    #[serde(rename = "classId")]
    class_id: u32,
    #[serde(rename = "searchFilter")]
    search_filter: String,
    #[serde(rename = "pageSize")]
    page_size: u32,
    index: u32,
    #[serde(rename = "gameVersionTypeId")]
    game_version_type_id: u32,
    #[serde(rename = "sortField")]
    sort_field: u32,
    #[serde(rename = "sortOrder")]
    sort_order: String,
}

#[derive(Debug, Serialize)]
struct SlugSearchParams {
    #[serde(rename = "gameId")]
    game_id: u32,
    slug: String,
}

#[derive(Debug, Serialize)]
struct BatchModsRequest {
    #[serde(rename = "modIds")]
    mod_ids: Vec<u32>,
}

/// Lightweight version info from a batch mod lookup, sufficient for update detection.
#[derive(Debug)]
pub struct BatchVersionCheck {
    pub addon_id: String,
    pub file_id: u32,
    pub version: String,
    pub display_name: String,
    pub released_at: String,
}

#[derive(Debug, Serialize)]
struct FingerprintsRequest {
    fingerprints: Vec<u32>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfFingerprintResponse {
    #[serde(rename = "isCacheBuilt")]
    is_cache_built: bool,
    #[serde(rename = "exactMatches")]
    exact_matches: Vec<CfFingerprintMatchEntry>,
    #[serde(rename = "exactFingerprints", default)]
    exact_fingerprints: Vec<u64>,
    #[serde(rename = "partialMatches")]
    partial_matches: Vec<CfFingerprintMatchEntry>,
    #[serde(rename = "installedFingerprints", default)]
    installed_fingerprints: Vec<u64>,
    #[serde(rename = "unmatchedFingerprints", default)]
    unmatched_fingerprints: Vec<u64>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct CfFingerprintMatchEntry {
    id: u32,
    file: CfFile,
    #[serde(rename = "latestFiles")]
    latest_files: Vec<CfFile>,
}

/// Results from a bulk fingerprint lookup against CurseForge.
#[derive(Debug)]
pub struct FingerprintMatchesResult {
    pub exact_matches: Vec<FingerprintMatch>,
    pub partial_matches: Vec<FingerprintMatch>,
    pub unmatched_fingerprints: Vec<u32>,
}

/// A single fingerprint match against a CurseForge addon.
#[derive(Debug, Clone)]
pub struct FingerprintMatch {
    pub mod_id: u32,
    pub file_id: u32,
    pub file_fingerprint: u32,
}

impl CurseForgeSource {
    /// Creates a new CurseForge source with the provided API key.
    pub fn new(api_key: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent(format!("wowctl/{}", env!("WOWCTL_VERSION")))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| WowctlError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_key,
            circuit_breaker: CircuitBreaker::new(),
        })
    }

    /// Gets addon information by numeric ID as raw JSON.
    pub async fn get_addon_by_id(&self, addon_id: &str) -> Result<serde_json::Value> {
        info!("Looking up addon by ID: {}", addon_id);
        let url = format!("{CURSEFORGE_API_BASE}/mods/{addon_id}");
        self.make_request_with_retry(&url).await
    }

    /// Gets typed addon information by numeric ID.
    pub async fn get_addon_info_by_id(&self, addon_id: &str) -> Result<AddonInfo> {
        info!("Looking up addon info by ID: {}", addon_id);
        let url = format!("{CURSEFORGE_API_BASE}/mods/{addon_id}");
        let mod_data: CfMod = self.make_request_with_retry(&url).await?;
        Ok(AddonInfo {
            id: mod_data.id.to_string(),
            name: mod_data.name,
            slug: mod_data.slug,
            description: mod_data.summary,
            download_count: mod_data.download_count.map(|d| d as u64),
            source: "curseforge".to_string(),
        })
    }

    /// Resolves a download URL for a file when the inline `downloadUrl` is null.
    ///
    /// Tries the dedicated `GET /download-url` endpoint first. If that also fails
    /// (common when `allowModDistribution` is not true), constructs the URL from
    /// the CurseForge CDN pattern used by other addon managers.
    async fn resolve_download_url(
        &self,
        addon_id: &str,
        file_id: u32,
        file_name: &str,
    ) -> Result<String> {
        let url = format!(
            "{CURSEFORGE_API_BASE}/mods/{addon_id}/files/{file_id}/download-url"
        );
        match self.make_request_with_retry::<String>(&url).await {
            Ok(download_url) if !download_url.is_empty() => {
                debug!("Got download URL from dedicated endpoint: {}", download_url);
                return Ok(download_url);
            }
            Ok(_) => {
                debug!(
                    "Dedicated download-url endpoint returned empty string for file {}",
                    file_id
                );
            }
            Err(e) => {
                debug!(
                    "Dedicated download-url endpoint failed for file {}: {}",
                    file_id, e
                );
            }
        }

        let cdn_url = build_cdn_url(file_id, file_name);
        info!("Using CDN URL for file {}: {}", file_id, cdn_url);
        Ok(cdn_url)
    }

    async fn make_request_with_retry<T: for<'de> Deserialize<'de>>(&self, url: &str) -> Result<T> {
        self.make_request_with_retry_params(url, &[] as &[(&str, &str)])
            .await
    }

    /// Records a request outcome with the circuit breaker.
    /// 404s (AddonNotFound) are not counted as failures.
    fn record_circuit_breaker_result<T>(&self, result: &Result<T>) {
        match result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(WowctlError::AddonNotFound(_)) | Err(WowctlError::CircuitBreakerOpen) => {}
            Err(_) => self.circuit_breaker.record_failure(),
        }
    }

    async fn make_request_with_retry_params<T: for<'de> Deserialize<'de>, P: Serialize + ?Sized>(
        &self,
        url: &str,
        params: &P,
    ) -> Result<T> {
        if !self.circuit_breaker.allow_request() {
            return Err(WowctlError::CircuitBreakerOpen);
        }
        let result = self.make_request_with_retry_params_inner(url, params).await;
        self.record_circuit_breaker_result(&result);
        result
    }

    async fn make_request_with_retry_params_inner<
        T: for<'de> Deserialize<'de>,
        P: Serialize + ?Sized,
    >(
        &self,
        url: &str,
        params: &P,
    ) -> Result<T> {
        let mut attempts = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            attempts += 1;
            debug!(
                "Making request to {} (attempt {}/{})",
                url, attempts, MAX_RETRIES
            );

            match self
                .client
                .get(url)
                .header("x-api-key", &self.api_key)
                .query(params)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    debug!("Response status: {}", status);

                    if status.is_success() {
                        let response_data: ApiResponse<T> = response.json().await.map_err(|e| {
                            WowctlError::Source(format!("Failed to parse API response: {e}"))
                        })?;
                        return Ok(response_data.data);
                    } else if status.as_u16() == 429 {
                        if attempts >= MAX_RETRIES {
                            return Err(WowctlError::Network(
                                "Rate limited by CurseForge API after multiple retries".to_string(),
                            ));
                        }
                        let retry_after_ms = Self::parse_retry_after(&response);
                        if let Some(delay_ms) = retry_after_ms {
                            warn!(
                                "Rate limited by CurseForge API, waiting {}s (from Retry-After header)...",
                                delay_ms / 1000
                            );
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }
                        warn!("Rate limited by CurseForge API, retrying with backoff...");
                    } else if status.as_u16() == 404 {
                        return Err(WowctlError::AddonNotFound(
                            "Addon not found on CurseForge".to_string(),
                        ));
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(WowctlError::Source(format!(
                            "CurseForge API error ({status}): {error_text}"
                        )));
                    }
                }
                Err(e) => {
                    warn!("Network error: {}", e);
                    if attempts >= MAX_RETRIES {
                        return Err(WowctlError::Network(format!(
                            "Failed to connect to CurseForge API after {MAX_RETRIES} attempts: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms *= 2;
        }
    }

    async fn make_paginated_request<T: for<'de> Deserialize<'de>, P: Serialize + ?Sized>(
        &self,
        url: &str,
        params: &P,
    ) -> Result<(T, CfPagination)> {
        if !self.circuit_breaker.allow_request() {
            return Err(WowctlError::CircuitBreakerOpen);
        }
        let result = self.make_paginated_request_inner(url, params).await;
        self.record_circuit_breaker_result(&result);
        result
    }

    async fn make_paginated_request_inner<T: for<'de> Deserialize<'de>, P: Serialize + ?Sized>(
        &self,
        url: &str,
        params: &P,
    ) -> Result<(T, CfPagination)> {
        let mut attempts = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            attempts += 1;
            debug!(
                "Making paginated request to {} (attempt {}/{})",
                url, attempts, MAX_RETRIES
            );

            match self
                .client
                .get(url)
                .header("x-api-key", &self.api_key)
                .query(params)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    debug!("Response status: {}", status);

                    if status.is_success() {
                        let response_data: PaginatedApiResponse<T> =
                            response.json().await.map_err(|e| {
                                WowctlError::Source(format!("Failed to parse API response: {e}"))
                            })?;
                        return Ok((response_data.data, response_data.pagination));
                    } else if status.as_u16() == 429 {
                        if attempts >= MAX_RETRIES {
                            return Err(WowctlError::Network(
                                "Rate limited by CurseForge API after multiple retries".to_string(),
                            ));
                        }
                        let retry_after_ms = Self::parse_retry_after(&response);
                        if let Some(delay_ms) = retry_after_ms {
                            warn!(
                                "Rate limited by CurseForge API, waiting {}s (from Retry-After header)...",
                                delay_ms / 1000
                            );
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }
                        warn!("Rate limited by CurseForge API, retrying with backoff...");
                    } else if status.as_u16() == 404 {
                        return Err(WowctlError::AddonNotFound(
                            "Addon not found on CurseForge".to_string(),
                        ));
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(WowctlError::Source(format!(
                            "CurseForge API error ({status}): {error_text}"
                        )));
                    }
                }
                Err(e) => {
                    warn!("Network error: {}", e);
                    if attempts >= MAX_RETRIES {
                        return Err(WowctlError::Network(format!(
                            "Failed to connect to CurseForge API after {MAX_RETRIES} attempts: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms *= 2;
        }
    }

    /// Parses the `Retry-After` header from a 429 response.
    /// Returns the delay in milliseconds, or `None` if the header is absent or unparseable.
    fn parse_retry_after(response: &reqwest::Response) -> Option<u64> {
        let header_value = response.headers().get("retry-after")?;
        let value_str = header_value.to_str().ok()?;
        Self::parse_retry_after_value(value_str)
    }

    /// Parses a `Retry-After` header value (seconds or HTTP-date) into a delay in milliseconds.
    fn parse_retry_after_value(value: &str) -> Option<u64> {
        let trimmed = value.trim();

        if let Ok(seconds) = trimmed.parse::<u64>() {
            return Some(seconds.max(1) * 1000);
        }

        if let Ok(date) = httpdate::parse_http_date(trimmed) {
            let now = std::time::SystemTime::now();
            if let Ok(duration) = date.duration_since(now) {
                return Some(duration.as_millis() as u64);
            }
        }

        None
    }

    async fn make_post_request_with_retry<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        if !self.circuit_breaker.allow_request() {
            return Err(WowctlError::CircuitBreakerOpen);
        }
        let result = self.make_post_request_with_retry_inner(url, body).await;
        self.record_circuit_breaker_result(&result);
        result
    }

    async fn make_post_request_with_retry_inner<T: for<'de> Deserialize<'de>, B: Serialize>(
        &self,
        url: &str,
        body: &B,
    ) -> Result<T> {
        let mut attempts = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            attempts += 1;
            debug!(
                "Making POST request to {} (attempt {}/{})",
                url, attempts, MAX_RETRIES
            );

            match self
                .client
                .post(url)
                .header("x-api-key", &self.api_key)
                .json(body)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    debug!("Response status: {}", status);

                    if status.is_success() {
                        let response_data: ApiResponse<T> = response.json().await.map_err(|e| {
                            WowctlError::Source(format!("Failed to parse API response: {e}"))
                        })?;
                        return Ok(response_data.data);
                    } else if status.as_u16() == 429 {
                        if attempts >= MAX_RETRIES {
                            return Err(WowctlError::Network(
                                "Rate limited by CurseForge API after multiple retries".to_string(),
                            ));
                        }
                        let retry_after_ms = Self::parse_retry_after(&response);
                        if let Some(delay_ms) = retry_after_ms {
                            warn!(
                                "Rate limited by CurseForge API, waiting {}s (from Retry-After header)...",
                                delay_ms / 1000
                            );
                            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                            continue;
                        }
                        warn!("Rate limited by CurseForge API, retrying with backoff...");
                    } else if status.as_u16() == 404 {
                        return Err(WowctlError::AddonNotFound(
                            "Resource not found on CurseForge".to_string(),
                        ));
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(WowctlError::Source(format!(
                            "CurseForge API error ({status}): {error_text}"
                        )));
                    }
                }
                Err(e) => {
                    warn!("Network error: {}", e);
                    if attempts >= MAX_RETRIES {
                        return Err(WowctlError::Network(format!(
                            "Failed to connect to CurseForge API after {MAX_RETRIES} attempts: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms *= 2;
        }
    }

    /// Fetches multiple mods in a single API call and returns the latest retail
    /// version info for each, keyed by addon ID string.
    pub async fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> Result<HashMap<String, BatchVersionCheck>> {
        if addon_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let mod_ids: Vec<u32> = addon_ids
            .iter()
            .filter_map(|id| id.parse::<u32>().ok())
            .collect();

        info!(
            "Batch fetching {} mods from CurseForge (channel: {})",
            mod_ids.len(),
            channel
        );
        let url = format!("{CURSEFORGE_API_BASE}/mods");
        let body = BatchModsRequest { mod_ids };
        let mods: Vec<CfMod> = self.make_post_request_with_retry(&url, &body).await?;

        let mut results = HashMap::new();
        for cf_mod in mods {
            let retail_file_id = cf_mod
                .latest_files_indexes
                .iter()
                .filter(|idx| idx.game_version_type_id == Some(WOW_RETAIL_VERSION_TYPE_ID))
                .filter(|idx| channel.includes_release_type(idx.release_type))
                .map(|idx| idx.file_id)
                .max();

            let retail_file_id = match retail_file_id {
                Some(id) => id,
                None => continue,
            };

            let latest_file = cf_mod
                .latest_files
                .as_ref()
                .and_then(|files| files.iter().find(|f| f.id == retail_file_id));

            let (version, display_name, released_at) = match latest_file {
                Some(file) => {
                    let version = self.extract_version_from_display_name(&file.display_name);
                    (version, file.display_name.clone(), file.file_date.clone())
                }
                None => continue,
            };

            results.insert(
                cf_mod.id.to_string(),
                BatchVersionCheck {
                    addon_id: cf_mod.id.to_string(),
                    file_id: retail_file_id,
                    version,
                    display_name,
                    released_at,
                },
            );
        }

        debug!(
            "Batch check returned version info for {} of {} addons",
            results.len(),
            addon_ids.len()
        );
        Ok(results)
    }

    /// Fetches multiple mods by ID in a single API call and returns `AddonInfo` for each.
    ///
    /// Uses `POST /v1/mods` to batch-resolve addon metadata, reducing per-dependency
    /// API calls from 2 (get_by_id + get_by_slug) to a single batch request.
    pub async fn get_addon_infos_batch(&self, addon_ids: &[String]) -> Result<Vec<AddonInfo>> {
        if addon_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mod_ids: Vec<u32> = addon_ids
            .iter()
            .filter_map(|id| id.parse::<u32>().ok())
            .collect();

        if mod_ids.is_empty() {
            return Ok(Vec::new());
        }

        info!(
            "Batch fetching {} addon infos from CurseForge",
            mod_ids.len()
        );
        let url = format!("{CURSEFORGE_API_BASE}/mods");
        let body = BatchModsRequest { mod_ids };
        let mods: Vec<CfMod> = self.make_post_request_with_retry(&url, &body).await?;

        let results: Vec<AddonInfo> = mods
            .into_iter()
            .map(|m| AddonInfo {
                id: m.id.to_string(),
                name: m.name,
                slug: m.slug,
                description: m.summary,
                download_count: m.download_count.map(|d| d as u64),
                source: "curseforge".to_string(),
            })
            .collect();

        debug!("Batch lookup returned {} addon infos", results.len());
        Ok(results)
    }

    /// Matches addon fingerprints against CurseForge's database in a single API call.
    ///
    /// Sends all fingerprints via `POST /v1/fingerprints` and returns both exact
    /// and partial matches. Each match maps a fingerprint to a CurseForge mod ID
    /// and the matched file ID. Use `get_addon_infos_batch()` to look up mod
    /// names/slugs for the returned mod IDs.
    pub async fn get_fingerprint_matches(
        &self,
        fingerprints: &[u32],
    ) -> Result<FingerprintMatchesResult> {
        if fingerprints.is_empty() {
            return Ok(FingerprintMatchesResult {
                exact_matches: Vec::new(),
                partial_matches: Vec::new(),
                unmatched_fingerprints: Vec::new(),
            });
        }

        info!(
            "Sending {} fingerprints to CurseForge for matching",
            fingerprints.len()
        );
        let url = format!("{CURSEFORGE_API_BASE}/fingerprints");
        let body = FingerprintsRequest {
            fingerprints: fingerprints.to_vec(),
        };

        let response: CfFingerprintResponse =
            self.make_post_request_with_retry(&url, &body).await?;

        let exact_matches: Vec<FingerprintMatch> = response
            .exact_matches
            .iter()
            .map(|m| FingerprintMatch {
                mod_id: m.id,
                file_id: m.file.id,
                file_fingerprint: m.file.file_fingerprint as u32,
            })
            .collect();

        let partial_matches: Vec<FingerprintMatch> = response
            .partial_matches
            .iter()
            .map(|m| FingerprintMatch {
                mod_id: m.id,
                file_id: m.file.id,
                file_fingerprint: m.file.file_fingerprint as u32,
            })
            .collect();

        let unmatched_fingerprints: Vec<u32> = response
            .unmatched_fingerprints
            .iter()
            .map(|&fp| fp as u32)
            .collect();

        debug!(
            "Fingerprint matches: {} exact, {} partial, {} unmatched",
            exact_matches.len(),
            partial_matches.len(),
            unmatched_fingerprints.len()
        );

        Ok(FingerprintMatchesResult {
            exact_matches,
            partial_matches,
            unmatched_fingerprints,
        })
    }

    /// Extracts a version string from a CurseForge file display name.
    ///
    /// Display names typically follow `<AddonName> <version>`, but the version
    /// part can contain spaces (e.g. "Plumber 1.8.8 b"). We find the first
    /// token that looks like a version (starts with a digit or 'v' + digit)
    /// and return everything from that point onward.
    pub fn extract_version(&self, display_name: &str) -> String {
        self.extract_version_from_display_name(display_name)
    }

    fn extract_version_from_display_name(&self, display_name: &str) -> String {
        let trimmed = display_name.trim();
        let parts: Vec<&str> = trimmed.split_whitespace().collect();

        if parts.len() <= 1 {
            return trimmed.to_string();
        }

        // If the first token looks like a version, the entire string is the version
        // (e.g. "1.8.8 b" with no addon name prefix).
        if Self::looks_like_version(parts[0]) {
            return trimmed.to_string();
        }

        for (i, part) in parts.iter().enumerate().skip(1) {
            if Self::looks_like_version(part) {
                return parts[i..].join(" ");
            }
        }

        // No version-like token found; assume first token is the addon name
        parts[1..].join(" ")
    }

    fn looks_like_version(s: &str) -> bool {
        if s.is_empty() {
            return false;
        }
        let bytes = s.as_bytes();
        bytes[0].is_ascii_digit()
            || (bytes[0] == b'v' && bytes.len() > 1 && bytes[1].is_ascii_digit())
    }
}

impl AddonSource for CurseForgeSource {
    async fn search(&self, query: &str, page: Option<u32>) -> Result<SearchResult> {
        let page_num = page.unwrap_or(1).max(1);
        let page_size = 20u32;
        let index = (page_num - 1) * page_size;

        info!(
            "Searching CurseForge for: {} (page {}, index {})",
            query, page_num, index
        );

        let url = format!("{CURSEFORGE_API_BASE}/mods/search");
        let params = SearchParams {
            game_id: WOW_GAME_ID,
            class_id: WOW_ADDONS_CLASS_ID,
            search_filter: query.to_string(),
            page_size,
            index,
            game_version_type_id: WOW_RETAIL_VERSION_TYPE_ID,
            sort_field: 2,
            sort_order: "desc".to_string(),
        };

        let (mods, pagination): (Vec<CfMod>, CfPagination) =
            self.make_paginated_request(&url, &params).await?;

        let addons: Vec<AddonInfo> = mods
            .into_iter()
            .map(|mod_data| AddonInfo {
                id: mod_data.id.to_string(),
                name: mod_data.name,
                slug: mod_data.slug,
                description: mod_data.summary,
                download_count: mod_data.download_count.map(|d| d as u64),
                source: "curseforge".to_string(),
            })
            .collect();

        debug!(
            "Found {} results (total: {})",
            addons.len(),
            pagination.total_count
        );
        Ok(SearchResult {
            addons,
            page: page_num,
            page_size: pagination.page_size,
            total_count: pagination.total_count,
        })
    }

    async fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<VersionInfo> {
        info!(
            "Getting latest version for addon ID: {} (channel: {})",
            addon_id, channel
        );

        let url = format!("{CURSEFORGE_API_BASE}/mods/{addon_id}/files");
        let params = [("gameVersionTypeId", WOW_RETAIL_VERSION_TYPE_ID.to_string())];
        let files: Vec<CfFile> = self.make_request_with_retry_params(&url, &params).await?;

        let latest_file = files
            .into_iter()
            .filter(|f| channel.includes_release_type(f.release_type))
            .max_by_key(|f| f.file_date.clone())
            .ok_or_else(|| {
                WowctlError::Source(format!(
                    "No compatible {channel} version found for WoW Retail"
                ))
            })?;

        let file_id = latest_file.id;

        let download_url = match latest_file.download_url {
            Some(url) => url,
            None => {
                debug!(
                    "File {} has no inline downloadUrl, resolving via fallback",
                    file_id
                );
                self.resolve_download_url(addon_id, file_id, &latest_file.file_name)
                    .await?
            }
        };

        let version = self.extract_version_from_display_name(&latest_file.display_name);
        let display_name = latest_file.display_name.clone();

        let dependencies = latest_file
            .dependencies
            .into_iter()
            .map(|dep| {
                let dependency_type = match dep.relation_type {
                    3 => DependencyType::Required,
                    2 => DependencyType::Optional,
                    1 => DependencyType::Embedded,
                    _ => DependencyType::Optional,
                };
                DependencyInfo {
                    addon_id: dep.mod_id.to_string(),
                    dependency_type,
                }
            })
            .collect();

        let modules: Vec<String> = latest_file.modules.iter().map(|m| m.name.clone()).collect();
        if !modules.is_empty() {
            debug!("File modules (canonical directories): {:?}", modules);
        }

        Ok(VersionInfo {
            file_id,
            version,
            display_name,
            download_url,
            file_name: latest_file.file_name,
            file_size: latest_file.file_length,
            game_versions: latest_file.game_versions,
            released_at: latest_file.file_date,
            dependencies,
            modules,
        })
    }

    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        info!("Downloading from: {}", download_url);

        let response = self
            .client
            .get(download_url)
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
        let content_length = response
            .headers()
            .get(reqwest::header::CONTENT_LENGTH)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(not set)")
            .to_string();
        let content_encoding = response
            .headers()
            .get(reqwest::header::CONTENT_ENCODING)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("(not set)")
            .to_string();

        info!(
            "Response: status={}, content-type={}, content-length={}, content-encoding={}",
            status, content_type, content_length, content_encoding
        );

        if !status.is_success() {
            return Err(WowctlError::Network(format!(
                "Download failed with status: {status}"
            )));
        }

        // Reject HTML error pages that CDNs sometimes serve with 200 OK
        if content_type.contains("text/html") || content_type.contains("text/plain") {
            return Err(WowctlError::Network(format!(
                "CDN returned {content_type} instead of a zip file — the download URL may be invalid: {download_url}"
            )));
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| WowctlError::Network(format!("Failed to read download: {e}")))?;

        info!("Downloaded {} bytes", bytes.len());

        // Log first and last bytes for diagnosing corrupted/truncated downloads
        if bytes.len() >= 16 {
            debug!("First 16 bytes: {:02x?}", &bytes[..16]);
            debug!(
                "Last 16 bytes: {:02x?}",
                &bytes[bytes.len() - 16..]
            );
        }
        // Validate ZIP magic bytes (PK\x03\x04) before writing to disk
        if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
            if bytes.len() < 1024 {
                // Dump small non-zip response body for debugging (likely an error page)
                debug!(
                    "Response body for invalid zip (small, {} bytes): {:?}",
                    bytes.len(),
                    String::from_utf8_lossy(&bytes)
                );
            }
            return Err(WowctlError::Extraction(format!(
                "Downloaded file is not a valid zip archive (bad magic bytes). \
                 Got {} bytes, first 4: {:02x?}. \
                 The CDN may have returned an error page. URL: {}",
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

        info!("Downloaded to: {}", destination.display());
        Ok(destination.to_path_buf())
    }

    async fn resolve_dependencies(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<Vec<String>> {
        debug!("Resolving dependencies for addon ID: {}", addon_id);

        let version_info = self.get_latest_version(addon_id, channel).await?;

        let required_deps: Vec<String> = version_info
            .dependencies
            .into_iter()
            .filter(|dep| dep.dependency_type == DependencyType::Required)
            .map(|dep| dep.addon_id)
            .collect();

        debug!("Found {} required dependencies", required_deps.len());
        Ok(required_deps)
    }

    async fn get_addon_by_slug(&self, slug: &str) -> Result<AddonInfo> {
        info!("Looking up addon by slug: {}", slug);

        let url = format!("{CURSEFORGE_API_BASE}/mods/search");
        let params = SlugSearchParams {
            game_id: WOW_GAME_ID,
            slug: slug.to_string(),
        };

        let mods: Vec<CfMod> = self.make_request_with_retry_params(&url, &params).await?;

        let mod_data = mods
            .into_iter()
            .find(|m| m.slug == slug)
            .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{slug}' not found")))?;

        Ok(AddonInfo {
            id: mod_data.id.to_string(),
            name: mod_data.name,
            slug: mod_data.slug,
            description: mod_data.summary,
            download_count: mod_data.download_count.map(|d| d as u64),
            source: "curseforge".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> CurseForgeSource {
        CurseForgeSource::new("test-key".to_string()).unwrap()
    }

    #[test]
    fn version_with_space_suffix() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("Plumber 1.8.8 b"),
            "1.8.8 b"
        );
    }

    #[test]
    fn version_only_display_name() {
        let s = source();
        assert_eq!(s.extract_version_from_display_name("1.8.8 b"), "1.8.8 b");
    }

    #[test]
    fn version_only_semver() {
        let s = source();
        assert_eq!(s.extract_version_from_display_name("10.2.5"), "10.2.5");
    }

    #[test]
    fn version_only_with_v_prefix() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("v2.0.0-beta1"),
            "v2.0.0-beta1"
        );
    }

    #[test]
    fn simple_semver() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("Angleur 2.7.85"),
            "2.7.85"
        );
    }

    #[test]
    fn version_with_dash() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("LiteMount 12.0.1-4"),
            "12.0.1-4"
        );
    }

    #[test]
    fn version_with_v_prefix() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("TomTom v4.2.22-release"),
            "v4.2.22-release"
        );
    }

    #[test]
    fn single_word_display_name() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("CityGuide.zip"),
            "CityGuide.zip"
        );
    }

    #[test]
    fn no_version_like_token() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("CityGuide CityGuide.zip"),
            "CityGuide.zip"
        );
    }

    #[test]
    fn multi_word_addon_name_with_version() {
        let s = source();
        assert_eq!(
            s.extract_version_from_display_name("My Knowledge Tracker 0.3.2"),
            "0.3.2"
        );
    }

    #[test]
    fn empty_string() {
        let s = source();
        assert_eq!(s.extract_version_from_display_name(""), "");
    }

    #[test]
    fn whitespace_only() {
        let s = source();
        assert_eq!(s.extract_version_from_display_name("   "), "");
    }

    #[test]
    fn retry_after_seconds() {
        assert_eq!(
            CurseForgeSource::parse_retry_after_value("30"),
            Some(30_000)
        );
    }

    #[test]
    fn retry_after_zero_clamps_to_one() {
        assert_eq!(CurseForgeSource::parse_retry_after_value("0"), Some(1_000));
    }

    #[test]
    fn retry_after_with_whitespace() {
        assert_eq!(
            CurseForgeSource::parse_retry_after_value("  60  "),
            Some(60_000)
        );
    }

    #[test]
    fn retry_after_garbage_returns_none() {
        assert_eq!(
            CurseForgeSource::parse_retry_after_value("not-a-number"),
            None
        );
    }

    #[test]
    fn retry_after_empty_returns_none() {
        assert_eq!(CurseForgeSource::parse_retry_after_value(""), None);
    }

    #[test]
    fn fingerprints_request_serializes_correctly() {
        let req = FingerprintsRequest {
            fingerprints: vec![123456789, 987654321, 0],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "fingerprints": [123456789, 987654321, 0] })
        );
    }

    #[test]
    fn fingerprint_response_deserializes_exact_match() {
        let json = serde_json::json!({
            "isCacheBuilt": true,
            "exactMatches": [{
                "id": 12345,
                "file": {
                    "id": 5001,
                    "displayName": "MyAddon 1.0.0",
                    "fileName": "MyAddon-1.0.0.zip",
                    "downloadUrl": "https://example.com/file.zip",
                    "fileLength": 50000,
                    "gameVersions": ["11.1.0"],
                    "dependencies": [],
                    "fileDate": "2025-01-01T00:00:00Z",
                    "releaseType": 1,
                    "fileFingerprint": 3456789012u64
                },
                "latestFiles": []
            }],
            "exactFingerprints": [3456789012u64],
            "partialMatches": [],
            "installedFingerprints": [3456789012u64],
            "unmatchedFingerprints": [999999u64]
        });
        let resp: CfFingerprintResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.exact_matches.len(), 1);
        assert_eq!(resp.exact_matches[0].id, 12345);
        assert_eq!(resp.exact_matches[0].file.id, 5001);
        assert_eq!(resp.exact_matches[0].file.file_fingerprint, 3456789012);
        assert_eq!(resp.partial_matches.len(), 0);
        assert_eq!(resp.unmatched_fingerprints, vec![999999]);
    }

    #[test]
    fn fingerprint_response_deserializes_partial_match() {
        let json = serde_json::json!({
            "isCacheBuilt": true,
            "exactMatches": [],
            "exactFingerprints": [],
            "partialMatches": [{
                "id": 67890,
                "file": {
                    "id": 6001,
                    "displayName": "Partial 2.0",
                    "fileName": "Partial-2.0.zip",
                    "downloadUrl": null,
                    "fileLength": 30000,
                    "gameVersions": [],
                    "dependencies": [],
                    "fileDate": "2025-06-01T00:00:00Z",
                    "releaseType": 2,
                    "fileFingerprint": 1111111111u64
                },
                "latestFiles": []
            }],
            "installedFingerprints": [],
            "unmatchedFingerprints": []
        });
        let resp: CfFingerprintResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.exact_matches.len(), 0);
        assert_eq!(resp.partial_matches.len(), 1);
        assert_eq!(resp.partial_matches[0].id, 67890);
        assert_eq!(resp.partial_matches[0].file.file_fingerprint, 1111111111);
    }

    #[test]
    fn fingerprint_response_handles_empty_result() {
        let json = serde_json::json!({
            "isCacheBuilt": true,
            "exactMatches": [],
            "exactFingerprints": [],
            "partialMatches": [],
            "installedFingerprints": [],
            "unmatchedFingerprints": [100, 200, 300]
        });
        let resp: CfFingerprintResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.exact_matches.len(), 0);
        assert_eq!(resp.partial_matches.len(), 0);
        assert_eq!(resp.unmatched_fingerprints, vec![100, 200, 300]);
    }

    #[test]
    fn cffile_deserializes_with_fingerprint() {
        let json = serde_json::json!({
            "id": 1001,
            "displayName": "TestAddon 3.2.1",
            "fileName": "test.zip",
            "downloadUrl": "https://example.com/test.zip",
            "fileLength": 12345,
            "gameVersions": ["11.0.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1,
            "fileFingerprint": 4294967295u64
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert_eq!(file.file_fingerprint, 4294967295);
    }

    #[test]
    fn cffile_deserializes_without_fingerprint() {
        let json = serde_json::json!({
            "id": 1001,
            "displayName": "TestAddon 3.2.1",
            "fileName": "test.zip",
            "downloadUrl": "https://example.com/test.zip",
            "fileLength": 12345,
            "gameVersions": ["11.0.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert_eq!(file.file_fingerprint, 0);
        assert!(file.modules.is_empty());
    }

    #[test]
    fn cffile_deserializes_with_modules() {
        let json = serde_json::json!({
            "id": 1001,
            "displayName": "WeakAuras 5.12.8",
            "fileName": "WeakAuras-5.12.8.zip",
            "downloadUrl": "https://example.com/wa.zip",
            "fileLength": 500000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1,
            "fileFingerprint": 123456,
            "modules": [
                { "name": "WeakAuras", "fingerprint": 111111 },
                { "name": "WeakAurasOptions", "fingerprint": 222222 },
                { "name": "WeakAurasModelPaths", "fingerprint": 333333 }
            ]
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert_eq!(file.modules.len(), 3);
        assert_eq!(file.modules[0].name, "WeakAuras");
        assert_eq!(file.modules[0].fingerprint, 111111);
        assert_eq!(file.modules[1].name, "WeakAurasOptions");
        assert_eq!(file.modules[2].name, "WeakAurasModelPaths");
    }

    #[test]
    fn cffile_modules_defaults_to_empty_when_absent() {
        let json = serde_json::json!({
            "id": 2001,
            "displayName": "OldAddon 1.0",
            "fileName": "old.zip",
            "downloadUrl": "https://example.com/old.zip",
            "fileLength": 1000,
            "gameVersions": ["10.0.0"],
            "dependencies": [],
            "fileDate": "2024-01-01T00:00:00Z",
            "releaseType": 1
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert!(file.modules.is_empty());
    }

    #[test]
    fn cfmodule_deserializes_without_fingerprint() {
        let json = serde_json::json!({ "name": "Details" });
        let module: CfModule = serde_json::from_value(json).unwrap();
        assert_eq!(module.name, "Details");
        assert_eq!(module.fingerprint, 0);
    }

    #[test]
    fn cffile_download_url_is_none_when_null() {
        let json = serde_json::json!({
            "id": 5001,
            "displayName": "Auctionator 11.0.12",
            "fileName": "Auctionator-11.0.12.zip",
            "downloadUrl": null,
            "fileLength": 800000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert!(file.download_url.is_none());
    }

    #[test]
    fn cffile_download_url_is_some_when_present() {
        let json = serde_json::json!({
            "id": 5002,
            "displayName": "Details 1.0",
            "fileName": "Details-1.0.zip",
            "downloadUrl": "https://edge.forgecdn.net/files/5002/Details-1.0.zip",
            "fileLength": 400000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert_eq!(
            file.download_url.as_deref(),
            Some("https://edge.forgecdn.net/files/5002/Details-1.0.zip")
        );
    }

    #[test]
    fn cffile_download_url_is_none_when_field_absent() {
        let json = serde_json::json!({
            "id": 5003,
            "displayName": "RestrictedAddon 2.0",
            "fileName": "RestrictedAddon-2.0.zip",
            "fileLength": 100000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2025-03-01T00:00:00Z",
            "releaseType": 1
        });
        let file: CfFile = serde_json::from_value(json).unwrap();
        assert!(file.download_url.is_none());
    }

    #[test]
    fn download_url_api_response_deserializes() {
        let json = serde_json::json!({
            "data": "https://edge.forgecdn.net/files/5877/543/Auctionator-11.0.12.zip"
        });
        let resp: ApiResponse<String> = serde_json::from_value(json).unwrap();
        assert_eq!(
            resp.data,
            "https://edge.forgecdn.net/files/5877/543/Auctionator-11.0.12.zip"
        );
    }

    #[test]
    fn download_url_api_response_empty_string() {
        let json = serde_json::json!({ "data": "" });
        let resp: ApiResponse<String> = serde_json::from_value(json).unwrap();
        assert!(resp.data.is_empty());
    }

    #[test]
    fn cdn_url_standard_file_id() {
        assert_eq!(
            build_cdn_url(5877543, "Auctionator-11.0.12.zip"),
            "https://edge.forgecdn.net/files/5877/543/Auctionator-11.0.12.zip"
        );
    }

    #[test]
    fn cdn_url_file_id_with_trailing_zeros() {
        assert_eq!(
            build_cdn_url(5100000, "TestAddon.zip"),
            "https://edge.forgecdn.net/files/5100/0/TestAddon.zip"
        );
    }

    #[test]
    fn cdn_url_small_file_id() {
        assert_eq!(
            build_cdn_url(1234, "SmallAddon.zip"),
            "https://edge.forgecdn.net/files/1/234/SmallAddon.zip"
        );
    }

    #[test]
    fn cdn_url_six_digit_file_id() {
        assert_eq!(
            build_cdn_url(123456, "Addon.zip"),
            "https://edge.forgecdn.net/files/123/456/Addon.zip"
        );
    }
}
