use crate::addon::{InstalledAddon, ReleaseChannel, VersionInfo};
use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use crate::sources::AddonSource;
use crate::sources::curseforge::CurseForgeSource;
use crate::utils::{
    backup_addon_dirs, check_disk_space, cleanup_backup_dirs, cleanup_temp_dir,
    extract_zip_to_temp, move_addon_dirs, restore_addon_dirs,
};
use dialoguer::Confirm;
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
    let api_key = config.get_api_key()?;
    let default_channel = config.resolve_channel(channel_override);
    let mut registry = Registry::load()?;

    let source = Arc::new(CurseForgeSource::new(api_key)?);

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
                    format!("Addon '{}' is not installed", slug).color_red()
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

    println!("Checking for updates...");
    let mut updates = Vec::new();
    let mut missed_addons = Vec::new();

    let addon_ids: Vec<&str> = addons_to_check
        .iter()
        .map(|a| a.addon_id.as_str())
        .collect();
    match source
        .get_latest_versions_batch(&addon_ids, default_channel)
        .await
    {
        Ok(batch_map) => {
            for installed in &addons_to_check {
                let addon_channel =
                    resolve_addon_channel(installed, channel_override, default_channel);
                if let Some(check) = batch_map.get(&installed.addon_id) {
                    let has_update = match installed.installed_file_id {
                        Some(installed_id) => check.file_id != installed_id,
                        None => check.version != installed.version,
                    };
                    if has_update {
                        updates.push(UpdateInfo {
                            slug: installed.slug.clone(),
                            name: installed.name.clone(),
                            current_version: installed.version.clone(),
                            new_version: check.version.clone(),
                            addon_id: installed.addon_id.clone(),
                            channel: addon_channel,
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
                debug!(
                    "Falling back to sequential check for {} addon(s) missing from batch",
                    missed_addons.len()
                );
                check_updates_sequential(
                    &source,
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
                "Batch update check failed ({}), falling back to sequential checks",
                e
            );
            check_updates_sequential(
                &source,
                &addons_to_check,
                channel_override,
                default_channel,
                &mut updates,
            )
            .await;
        }
    }

    // Fix version strings that were extracted with an older heuristic.
    let fixed = fix_version_strings(&source, &mut registry, &addons_to_check);

    // Refresh stale registry entries that are missing key metadata fields,
    // even when the installed version is already current.
    let stale_count = refresh_stale_metadata(
        &source,
        &mut registry,
        &addons_to_check,
        channel_override,
        default_channel,
    )
    .await;

    if fixed + stale_count > 0 {
        registry.save()?;
    }

    if updates.is_empty() {
        println!("{}", "All addons are up to date.".color_green());
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
        let source = Arc::clone(&source);
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
                println!("{}", format!("failed: {}", e).color_red());
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
                installed.installed_file_id = Some(download.version_info.file_id);
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
    source: &CurseForgeSource,
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
                let has_update = match installed.installed_file_id {
                    Some(installed_id) => version_info.file_id != installed_id,
                    None => version_info.version != installed.version,
                };
                if has_update {
                    updates.push(UpdateInfo {
                        slug: installed.slug.clone(),
                        name: installed.name.clone(),
                        current_version: installed.version.clone(),
                        new_version: version_info.version,
                        addon_id: installed.addon_id.clone(),
                        channel: addon_channel,
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

fn needs_metadata_refresh(addon: &InstalledAddon) -> bool {
    addon.installed_file_id.is_none()
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
    source: &CurseForgeSource,
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
                    entry.installed_file_id = Some(version_info.file_id);
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
            "Refreshed metadata for {} addon(s) with outdated registry entries.",
            refreshed
        );
    }

    refreshed
}
