use crate::addon::{InstalledAddon, ReleaseChannel, VersionInfo};
use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use crate::sources::{AddonSource, AnySource, BatchVersionCheck, SourceKind, build_source};
use crate::sources::curseforge::CurseForgeSource;
use crate::utils::{
    backup_addon_dirs, check_disk_space, cleanup_backup_dirs, cleanup_temp_dir,
    extract_zip_to_temp, move_addon_dirs, restore_addon_dirs,
};
use dialoguer::Confirm;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{debug, warn};

const MAX_CONCURRENT_DOWNLOADS: usize = 3;

struct UpdateInfo {
    slug: String,
    name: String,
    current_version: String,
    new_version: String,
    addon_id: String,
    channel: ReleaseChannel,
    kind: SourceKind,
}

struct UpdateDownload {
    slug: String,
    name: String,
    channel: ReleaseChannel,
    version_info: VersionInfo,
    temp_zip: PathBuf,
    temp_extract_dir: PathBuf,
    extracted_dirs: Vec<String>,
}

pub async fn update(
    addon: Option<&str>,
    auto: bool,
    auto_only: bool,
    channel_override: Option<ReleaseChannel>,
) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let default_channel = config.resolve_channel(channel_override);
    let mut registry = Registry::load()?;

    let addons_to_check: Vec<_> = match addon {
        Some(slug) => {
            let installed = registry.get(slug).cloned();
            if let Some(addon) = installed {
                if addon.is_ignored() {
                    println!(
                        "{} is ignored. Use {} to include it in update checks.",
                        slug.color_cyan(),
                        "wowctl unignore".color_bold()
                    );
                    return Ok(());
                }
                vec![addon]
            } else {
                println!(
                    "{}",
                    format!("Addon '{slug}' is not installed").color_red()
                );
                return Ok(());
            }
        }
        None => {
            let all: Vec<_> = registry.list_all().into_iter().cloned().collect();
            let (ignored, active): (Vec<_>, Vec<_>) = all.into_iter().partition(|a| a.is_ignored());
            if !ignored.is_empty() {
                debug!("Skipping {} ignored addon(s)", ignored.len());
            }
            if auto_only {
                let (auto_enabled, skipped): (Vec<_>, Vec<_>) =
                    active.into_iter().partition(|a| a.is_auto_update());
                if !skipped.is_empty() {
                    debug!("Skipping {} addon(s) without auto-update", skipped.len());
                }
                if auto_enabled.is_empty() {
                    println!(
                        "No addons have auto-update enabled. Use {} to enable it.",
                        "wowctl auto-update <slug>".color_bold()
                    );
                    return Ok(());
                }
                auto_enabled
            } else {
                active
            }
        }
    };

    if addons_to_check.is_empty() {
        println!("No managed addons to update.");
        return Ok(());
    }

    let (groups, unknown) = group_by_source(&addons_to_check);
    let mut any_skipped = !unknown.is_empty();
    for addon in &unknown {
        println!(
            "  {} Skipping {}: unknown source '{}'",
            "Warning:".color_yellow(),
            addon.slug.color_cyan(),
            addon.source
        );
    }

    println!("Checking for updates...");
    let mut updates = Vec::new();
    let mut sources: HashMap<SourceKind, Arc<AnySource>> = HashMap::new();
    let mut fixed = 0;
    let mut stale_count = 0;

    for (kind, group) in &groups {
        // One Source's missing credentials or outage must not block the other
        // (user stories 8 and 20).
        let source = match build_source(*kind, &config) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                println!(
                    "  {} Skipping {} {} addon(s): {}",
                    "Warning:".color_yellow(),
                    group.len(),
                    kind,
                    e
                );
                any_skipped = true;
                continue;
            }
        };
        sources.insert(*kind, Arc::clone(&source));

        let addon_ids: Vec<&str> = group.iter().map(|a| a.addon_id.as_str()).collect();
        match source
            .get_latest_versions_batch(&addon_ids, default_channel)
            .await
        {
            Ok(batch_map) => {
                let mut missed_addons = Vec::new();
                for installed in group {
                    let addon_channel =
                        resolve_addon_channel(installed, channel_override, default_channel);
                    if let Some(check) = batch_map.get(&installed.addon_id) {
                        if is_update_available(installed, check) {
                            updates.push(UpdateInfo {
                                slug: installed.slug.clone(),
                                name: installed.name.clone(),
                                current_version: installed.version.clone(),
                                new_version: check.version.clone(),
                                addon_id: installed.addon_id.clone(),
                                channel: addon_channel,
                                kind: *kind,
                            });
                        }
                    } else {
                        debug!(
                            "Addon {} not in batch result, will check individually",
                            installed.slug
                        );
                        missed_addons.push(installed.clone());
                    }
                }
                if !missed_addons.is_empty() {
                    check_updates_sequential(
                        &source,
                        *kind,
                        &missed_addons,
                        channel_override,
                        default_channel,
                        &mut updates,
                    )
                    .await;
                }
            }
            Err(e) => {
                warn!(
                    "Batch update check for {} failed ({}), falling back to sequential checks",
                    kind, e
                );
                check_updates_sequential(
                    &source,
                    *kind,
                    group,
                    channel_override,
                    default_channel,
                    &mut updates,
                )
                .await;
            }
        }

        // CurseForge-only repair: version strings extracted with an older heuristic.
        if let AnySource::CurseForge(cf) = source.as_ref() {
            fixed += fix_version_strings(cf, &mut registry, group);
        }

        stale_count += refresh_stale_metadata(
            &source,
            &mut registry,
            group,
            channel_override,
            default_channel,
        )
        .await;
    }

    if fixed + stale_count > 0 {
        registry.save()?;
    }

    if updates.is_empty() {
        if any_skipped {
            println!("{}", "Remaining addons are up to date.".color_green());
        } else {
            println!("{}", "All addons are up to date.".color_green());
        }
        return Ok(());
    }

    println!();
    println!("{}", "Updates available:".color_bold());
    for update in &updates {
        println!(
            "  {}  {} → {}",
            update.slug.color_cyan(),
            update.current_version.color_dimmed(),
            update.new_version.color_green()
        );
    }
    println!();

    let should_update = if auto {
        true
    } else {
        Confirm::new()
            .with_prompt(format!("Install {} update(s)?", updates.len()))
            .default(true)
            .interact()
            .unwrap_or(false)
    };

    if !should_update {
        println!("Update cancelled.");
        return Ok(());
    }

    // Concurrent download + extract phase (up to MAX_CONCURRENT_DOWNLOADS in parallel)
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let mut download_handles = Vec::new();

    for update in updates {
        let source = Arc::clone(
            sources
                .get(&update.kind)
                .expect("update only queued for sources that were built"),
        );
        let sem = Arc::clone(&semaphore);
        let addon_dir_clone = addon_dir.clone();

        download_handles.push(tokio::spawn(async move {
            let addon_name = update.name.clone();
            let result = async {
                let _permit = sem
                    .acquire()
                    .await
                    .map_err(|_| WowctlError::Source("Download semaphore closed".to_string()))?;

                let version_info = source
                    .get_latest_version(&update.addon_id, update.channel)
                    .await?;
                check_disk_space(&addon_dir_clone, version_info.file_size)?;

                let temp_zip = std::env::temp_dir().join(format!("{}.zip", update.slug));
                source
                    .download(&version_info.download_url, &temp_zip)
                    .await?;

                let (temp_extract_dir, extracted_dirs) = match extract_zip_to_temp(&temp_zip) {
                    Ok(result) => result,
                    Err(e) => {
                        let _ = std::fs::remove_file(&temp_zip);
                        return Err(e);
                    }
                };

                Ok::<_, WowctlError>(UpdateDownload {
                    slug: update.slug,
                    name: update.name,
                    channel: update.channel,
                    version_info,
                    temp_zip,
                    temp_extract_dir,
                    extracted_dirs,
                })
            }
            .await;

            (addon_name, result)
        }));
    }

    let mut successful_downloads = Vec::new();
    for handle in download_handles {
        match handle.await {
            Ok((_, Ok(download))) => successful_downloads.push(download),
            Ok((name, Err(e))) => {
                println!(
                    "  {} Failed to download {}: {}",
                    "Warning:".color_yellow(),
                    name,
                    e
                );
            }
            Err(join_err) => {
                println!(
                    "  {} Download task failed: {}",
                    "Warning:".color_yellow(),
                    join_err
                );
            }
        }
    }

    if successful_downloads.is_empty() {
        println!("{}", "All downloads failed.".color_red());
        return Ok(());
    }

    // Sequential backup + move + registry update phase
    for download in successful_downloads {
        print!(
            "Updating {} to {}... ",
            download.name.color_cyan(),
            download.version_info.version.color_green()
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());

        match apply_update(&mut registry, &addon_dir, download) {
            Ok(_) => {
                println!("{}", "done.".color_green());
            }
            Err(e) => {
                println!("{}", format!("failed: {e}").color_red());
            }
        }
    }

    registry.save()?;
    println!("{}", "Updates complete!".color_green().color_bold());

    Ok(())
}

fn apply_update(
    registry: &mut Registry,
    addon_dir: &std::path::Path,
    download: UpdateDownload,
) -> Result<()> {
    let new_directories = if download.version_info.modules.is_empty() {
        download.extracted_dirs.clone()
    } else {
        debug!(
            "Using CurseForge modules for directory list: {:?} (zip had: {:?})",
            download.version_info.modules, download.extracted_dirs
        );
        download.version_info.modules.clone()
    };

    if let Some(mut installed) = registry.get(&download.slug).cloned() {
        let backed_up = backup_addon_dirs(addon_dir, &installed.directories)?;

        match move_addon_dirs(
            &download.temp_extract_dir,
            addon_dir,
            &download.extracted_dirs,
        ) {
            Ok(_) => {
                cleanup_backup_dirs(addon_dir, &backed_up);
                installed.version = download.version_info.version;
                installed.directories = new_directories;
                installed.installed_file_id = download.version_info.file_id;
                installed.external_release_id = download.version_info.external_release_id.clone();
                installed.display_name = Some(download.version_info.display_name);
                installed.game_versions = Some(download.version_info.game_versions);
                installed.released_at = Some(download.version_info.released_at);
                if download.channel != ReleaseChannel::Stable {
                    installed.channel = Some(download.channel);
                }
                registry.add(installed);
            }
            Err(e) => {
                restore_addon_dirs(addon_dir, &backed_up);
                let _ = cleanup_temp_dir(&download.temp_extract_dir);
                let _ = std::fs::remove_file(&download.temp_zip);
                return Err(e);
            }
        }
    }

    cleanup_temp_dir(&download.temp_extract_dir)?;
    std::fs::remove_file(&download.temp_zip)?;

    Ok(())
}

/// Resolves the effective channel for an addon: CLI override > addon's stored channel > default.
fn resolve_addon_channel(
    installed: &InstalledAddon,
    cli_override: Option<ReleaseChannel>,
    default_channel: ReleaseChannel,
) -> ReleaseChannel {
    cli_override
        .or(installed.channel)
        .unwrap_or(default_channel)
}

async fn check_updates_sequential(
    source: &AnySource,
    kind: SourceKind,
    addons: &[InstalledAddon],
    channel_override: Option<ReleaseChannel>,
    default_channel: ReleaseChannel,
    updates: &mut Vec<UpdateInfo>,
) {
    for installed in addons {
        let addon_channel = resolve_addon_channel(installed, channel_override, default_channel);
        debug!(
            "Checking {} for updates (channel: {})",
            installed.slug, addon_channel
        );

        match source
            .get_latest_version(&installed.addon_id, addon_channel)
            .await
        {
            Ok(version_info) => {
                let check =
                    BatchVersionCheck::from_version_info(&installed.addon_id, &version_info);
                if is_update_available(installed, &check) {
                    updates.push(UpdateInfo {
                        slug: installed.slug.clone(),
                        name: installed.name.clone(),
                        current_version: installed.version.clone(),
                        new_version: check.version.clone(),
                        addon_id: installed.addon_id.clone(),
                        channel: addon_channel,
                        kind,
                    });
                }
            }
            Err(e) => {
                println!(
                    "  {} Failed to check {}: {}",
                    "Warning:".color_yellow(),
                    installed.name,
                    e
                );
            }
        }
    }
}

/// Determines whether a newer release is available by comparing the strongest
/// release identity both sides share: external release ID (Wago), then numeric
/// file ID (CurseForge), then the version string.
fn is_update_available(
    installed: &InstalledAddon,
    latest: &BatchVersionCheck,
) -> bool {
    match (
        installed.external_release_id.as_deref(),
        latest.external_release_id.as_deref(),
    ) {
        (Some(a), Some(b)) => a != b,
        _ => match (installed.installed_file_id, latest.file_id) {
            (Some(a), Some(b)) => a != b,
            _ => installed.version != latest.version,
        },
    }
}

/// A registry entry is stale when it lacks the release identity its Source
/// uses for update detection.
fn needs_metadata_refresh(addon: &InstalledAddon) -> bool {
    match addon.source.parse::<SourceKind>() {
        Ok(SourceKind::Wago) => addon.external_release_id.is_none(),
        _ => addon.installed_file_id.is_none(),
    }
}

/// Groups addons by their registry Source in deterministic order
/// (CurseForge, then Wago). Addons with an unrecognized source string are
/// returned separately so the caller can warn and skip them.
fn group_by_source(
    addons: &[InstalledAddon],
) -> (Vec<(SourceKind, Vec<InstalledAddon>)>, Vec<InstalledAddon>) {
    let mut unknown = Vec::new();
    let mut cf = Vec::new();
    let mut wago = Vec::new();
    for addon in addons {
        match addon.source.parse::<SourceKind>() {
            Ok(SourceKind::CurseForge) => cf.push(addon.clone()),
            Ok(SourceKind::Wago) => wago.push(addon.clone()),
            Err(_) => unknown.push(addon.clone()),
        }
    }
    let mut groups = Vec::new();
    if !cf.is_empty() {
        groups.push((SourceKind::CurseForge, cf));
    }
    if !wago.is_empty() {
        groups.push((SourceKind::Wago, wago));
    }
    (groups, unknown)
}

/// Re-extracts version strings from stored display names using the current
/// extraction logic, fixing entries that were written with an older heuristic.
/// Returns the number of entries corrected (no API calls needed).
fn fix_version_strings(
    source: &CurseForgeSource,
    registry: &mut Registry,
    addons: &[InstalledAddon],
) -> usize {
    let mut fixed = 0;
    for installed in addons {
        if let Some(ref display_name) = installed.display_name {
            let correct_version = source.extract_version(display_name);
            if correct_version != installed.version {
                debug!(
                    "Fixing version for {}: {:?} -> {:?} (from display_name {:?})",
                    installed.slug, installed.version, correct_version, display_name
                );
                if let Some(mut entry) = registry.get(&installed.slug).cloned() {
                    entry.version = correct_version;
                    registry.add(entry);
                    fixed += 1;
                }
            }
        }
    }
    fixed
}

/// Fetches current version info for addons with stale metadata and updates their
/// registry entries in place. Returns the number of entries refreshed.
async fn refresh_stale_metadata(
    source: &AnySource,
    registry: &mut Registry,
    addons: &[InstalledAddon],
    channel_override: Option<ReleaseChannel>,
    default_channel: ReleaseChannel,
) -> usize {
    let stale: Vec<_> = addons
        .iter()
        .filter(|a| needs_metadata_refresh(a))
        .collect();
    if stale.is_empty() {
        return 0;
    }

    debug!(
        "Found {} addon(s) with stale metadata, refreshing",
        stale.len()
    );
    let mut refreshed = 0;

    for installed in stale {
        let addon_channel = resolve_addon_channel(installed, channel_override, default_channel);
        match source
            .get_latest_version(&installed.addon_id, addon_channel)
            .await
        {
            Ok(version_info) => {
                if let Some(mut entry) = registry.get(&installed.slug).cloned() {
                    entry.version = version_info.version;
                    entry.installed_file_id = version_info.file_id;
                    entry.external_release_id = version_info.external_release_id.clone();
                    entry.display_name = Some(version_info.display_name);
                    entry.game_versions = Some(version_info.game_versions);
                    entry.released_at = Some(version_info.released_at);
                    if addon_channel != ReleaseChannel::Stable {
                        entry.channel = Some(addon_channel);
                    }
                    registry.add(entry);
                    refreshed += 1;
                }
            }
            Err(e) => {
                debug!("Failed to refresh metadata for {}: {}", installed.slug, e);
            }
        }
    }

    if refreshed > 0 {
        println!(
            "Refreshed metadata for {refreshed} addon(s) with outdated registry entries."
        );
    }

    refreshed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_installed(
        file_id: Option<u32>,
        release_id: Option<&str>,
        version: &str,
    ) -> InstalledAddon {
        InstalledAddon {
            name: "Test".to_string(),
            slug: "test".to_string(),
            version: version.to_string(),
            source: "curseforge".to_string(),
            addon_id: "1".to_string(),
            directories: vec![],
            is_dependency: false,
            required_by: vec![],
            installed_file_id: file_id,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: None,
            auto_update: None,
            external_release_id: release_id.map(String::from),
        }
    }

    fn make_check(
        file_id: Option<u32>,
        release_id: Option<&str>,
        version: &str,
    ) -> BatchVersionCheck {
        BatchVersionCheck {
            addon_id: "1".to_string(),
            file_id,
            external_release_id: release_id.map(String::from),
            version: version.to_string(),
            display_name: version.to_string(),
            released_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn update_detected_by_external_release_id() {
        let installed = make_installed(None, Some("100"), "1.0");
        let latest = make_check(None, Some("200"), "1.0");
        assert!(is_update_available(&installed, &latest));
    }

    #[test]
    fn no_update_when_external_release_id_matches() {
        let installed = make_installed(None, Some("100"), "1.0");
        // Version string differs but release id matches: id wins.
        let latest = make_check(None, Some("100"), "1.1");
        assert!(!is_update_available(&installed, &latest));
    }

    #[test]
    fn update_detected_by_file_id() {
        let installed = make_installed(Some(10), None, "1.0");
        let latest = make_check(Some(11), None, "1.0");
        assert!(is_update_available(&installed, &latest));
    }

    #[test]
    fn no_update_when_file_id_matches() {
        let installed = make_installed(Some(10), None, "1.0");
        let latest = make_check(Some(10), None, "1.1");
        assert!(!is_update_available(&installed, &latest));
    }

    #[test]
    fn falls_back_to_version_comparison() {
        let installed = make_installed(None, None, "1.0");
        assert!(is_update_available(
            &installed,
            &make_check(None, None, "1.1")
        ));
        assert!(!is_update_available(
            &installed,
            &make_check(None, None, "1.0")
        ));
    }

    #[test]
    fn mixed_identity_falls_back_to_version() {
        // Installed pre-dates file-id tracking, latest has one: only versions are comparable.
        let installed = make_installed(None, None, "1.0");
        let latest = make_check(Some(11), None, "1.0");
        assert!(!is_update_available(&installed, &latest));
    }

    fn make_sourced(slug: &str, source: &str) -> InstalledAddon {
        let mut a = make_installed(None, None, "1.0");
        a.slug = slug.to_string();
        a.name = slug.to_string();
        a.source = source.to_string();
        a
    }

    #[test]
    fn group_by_source_splits_and_orders() {
        let addons = vec![
            make_sourced("w1", "wago"),
            make_sourced("c1", "curseforge"),
            make_sourced("w2", "wago"),
        ];
        let (groups, unknown) = group_by_source(&addons);
        assert!(unknown.is_empty());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, SourceKind::CurseForge);
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[1].0, SourceKind::Wago);
        assert_eq!(groups[1].1.len(), 2);
    }

    #[test]
    fn group_by_source_flags_unknown_sources() {
        let addons = vec![
            make_sourced("c1", "curseforge"),
            make_sourced("x1", "wowinterface"),
        ];
        let (groups, unknown) = group_by_source(&addons);
        assert_eq!(groups.len(), 1);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].slug, "x1");
    }

    #[test]
    fn group_by_source_empty_input() {
        let (groups, unknown) = group_by_source(&[]);
        assert!(groups.is_empty());
        assert!(unknown.is_empty());
    }

    #[test]
    fn wago_needs_refresh_when_release_id_missing() {
        let mut a = make_sourced("w1", "wago");
        a.external_release_id = None;
        a.installed_file_id = None;
        assert!(needs_metadata_refresh(&a));
        a.external_release_id = Some("123".to_string());
        assert!(!needs_metadata_refresh(&a));
    }

    #[test]
    fn curseforge_needs_refresh_when_file_id_missing() {
        let mut a = make_sourced("c1", "curseforge");
        a.installed_file_id = None;
        assert!(needs_metadata_refresh(&a));
        a.installed_file_id = Some(42);
        assert!(!needs_metadata_refresh(&a));
    }

    #[test]
    fn batch_check_from_version_info() {
        let v = crate::addon::VersionInfo {
            file_id: None,
            external_release_id: Some("12345".to_string()),
            version: "2.0".to_string(),
            display_name: "2.0".to_string(),
            download_url: "https://example.com/a.zip".to_string(),
            file_name: "a.zip".to_string(),
            file_size: 0,
            game_versions: vec![],
            released_at: "2026-01-01T00:00:00Z".to_string(),
            dependencies: vec![],
            modules: vec![],
        };
        let check = BatchVersionCheck::from_version_info("rNkynwKa", &v);
        assert_eq!(check.addon_id, "rNkynwKa");
        assert_eq!(check.file_id, None);
        assert_eq!(check.external_release_id, Some("12345".to_string()));
        assert_eq!(check.version, "2.0");
    }
}
