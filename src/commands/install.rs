use crate::addon::{AddonInfo, InstalledAddon, ReleaseChannel, VersionInfo};
use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use crate::sources::AddonSource;
use crate::sources::curseforge::CurseForgeSource;
use crate::utils::{
    check_directory_conflicts, check_disk_space, cleanup_temp_dir, extract_slug_from_url,
    extract_zip_to_temp, move_addon_dirs,
};
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::debug;

const MAX_CONCURRENT_DOWNLOADS: usize = 3;

struct DownloadedAddon {
    addon_info: AddonInfo,
    is_dependency: bool,
    version_info: VersionInfo,
    temp_zip: PathBuf,
    temp_extract_dir: PathBuf,
    extracted_dirs: Vec<String>,
}

pub async fn install(addon: &str, channel_override: Option<ReleaseChannel>) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let api_key = config.get_api_key()?;
    let channel = config.resolve_channel(channel_override);

    let slug = if addon.starts_with("http") {
        extract_slug_from_url(addon)?
    } else {
        addon.to_string()
    };

    let source = Arc::new(CurseForgeSource::new(api_key)?);
    let mut registry = Registry::load()?;

    if let Some(existing) = registry.get(&slug) {
        println!(
            "{} is already installed (version {})",
            existing.name.color_cyan(),
            existing.version.color_green()
        );
        return Ok(());
    }

    let addon_info = source.get_addon_by_slug(&slug).await?;

    let channel_label = if channel != ReleaseChannel::Stable {
        format!(" ({channel})")
    } else {
        String::new()
    };
    println!(
        "Installing {}{}...",
        addon_info.name.color_cyan(),
        channel_label
    );

    let mut to_install = VecDeque::new();
    let mut visited = HashSet::new();

    to_install.push_back((addon_info.clone(), false));
    visited.insert(addon_info.id.clone());

    let mut install_plan = Vec::new();

    while let Some((current_addon, is_dep)) = to_install.pop_front() {
        if registry.get(&current_addon.slug).is_some() {
            debug!("Addon {} already installed, skipping", current_addon.slug);
            continue;
        }

        let dep_ids = source
            .resolve_dependencies(&current_addon.id, channel)
            .await?;

        let new_dep_ids: Vec<String> = dep_ids
            .into_iter()
            .filter(|id| {
                if visited.contains(id) {
                    return false;
                }
                if registry.addons.values().any(|a| a.addon_id == *id) {
                    debug!("Dependency {} already installed", id);
                    return false;
                }
                true
            })
            .collect();

        for id in &new_dep_ids {
            visited.insert(id.clone());
        }

        if !new_dep_ids.is_empty() {
            let dep_infos = source
                .get_addon_infos_batch(&new_dep_ids)
                .await
                .map_err(|e| {
                    WowctlError::Dependency(format!("Failed to resolve dependencies: {e}"))
                })?;

            for dep_info in dep_infos {
                to_install.push_back((dep_info, true));
            }
        }

        install_plan.push((current_addon, is_dep));
    }

    // Concurrent download + extract phase (up to MAX_CONCURRENT_DOWNLOADS in parallel)
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_DOWNLOADS));
    let mut download_handles = Vec::new();

    for (addon_to_install, is_dep) in install_plan {
        let source = Arc::clone(&source);
        let sem = Arc::clone(&semaphore);
        let addon_dir_clone = addon_dir.clone();

        download_handles.push(tokio::spawn(async move {
            let _permit = sem
                .acquire()
                .await
                .map_err(|_| WowctlError::Source("Download semaphore closed".to_string()))?;

            let version_info = source
                .get_latest_version(&addon_to_install.id, channel)
                .await?;
            check_disk_space(&addon_dir_clone, version_info.file_size)?;

            let temp_zip = std::env::temp_dir().join(format!("{}.zip", addon_to_install.slug));
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

            Ok::<_, WowctlError>(DownloadedAddon {
                addon_info: addon_to_install,
                is_dependency: is_dep,
                version_info,
                temp_zip,
                temp_extract_dir,
                extracted_dirs,
            })
        }));
    }

    let mut downloads = Vec::new();
    for handle in download_handles {
        match handle.await {
            Ok(Ok(result)) => downloads.push(result),
            Ok(Err(e)) => {
                cleanup_downloaded(&downloads);
                return Err(e);
            }
            Err(join_err) => {
                cleanup_downloaded(&downloads);
                return Err(WowctlError::Source(format!(
                    "Download task failed: {join_err}"
                )));
            }
        }
    }

    // Sequential file move + registry update phase
    for downloaded in downloads {
        let prefix = if downloaded.is_dependency {
            "  Installing dependency: "
        } else {
            ""
        };
        let version_display = if downloaded.version_info.version.starts_with('v') {
            downloaded.version_info.version.clone()
        } else {
            format!("v{}", downloaded.version_info.version)
        };
        print!(
            "{}{} {}... ",
            prefix,
            downloaded.addon_info.name.color_cyan(),
            version_display.color_green()
        );
        let _ = std::io::Write::flush(&mut std::io::stdout());

        let directories = if downloaded.version_info.modules.is_empty() {
            downloaded.extracted_dirs.clone()
        } else {
            debug!(
                "Using CurseForge modules for directory list: {:?} (zip had: {:?})",
                downloaded.version_info.modules, downloaded.extracted_dirs
            );
            downloaded.version_info.modules.clone()
        };

        if let Err(e) = check_directory_conflicts(&registry, &directories, None) {
            let _ = cleanup_temp_dir(&downloaded.temp_extract_dir);
            let _ = std::fs::remove_file(&downloaded.temp_zip);
            return Err(e);
        }

        if let Err(e) = move_addon_dirs(
            &downloaded.temp_extract_dir,
            &addon_dir,
            &downloaded.extracted_dirs,
        ) {
            let _ = cleanup_temp_dir(&downloaded.temp_extract_dir);
            let _ = std::fs::remove_file(&downloaded.temp_zip);
            return Err(e);
        }

        cleanup_temp_dir(&downloaded.temp_extract_dir)?;
        std::fs::remove_file(&downloaded.temp_zip)?;

        let addon_channel = if channel != ReleaseChannel::Stable {
            Some(channel)
        } else {
            None
        };
        let installed = InstalledAddon {
            name: downloaded.addon_info.name.clone(),
            slug: downloaded.addon_info.slug.clone(),
            version: downloaded.version_info.version,
            source: downloaded.addon_info.source.clone(),
            addon_id: downloaded.addon_info.id.clone(),
            directories,
            is_dependency: downloaded.is_dependency,
            required_by: if downloaded.is_dependency {
                vec![slug.clone()]
            } else {
                vec![]
            },
            installed_file_id: Some(downloaded.version_info.file_id),
            display_name: Some(downloaded.version_info.display_name),
            channel: addon_channel,
            ignored: None,
            game_versions: Some(downloaded.version_info.game_versions),
            released_at: Some(downloaded.version_info.released_at),
            auto_update: None,
        };

        registry.add(installed);

        println!("{}", "done.".color_green());
    }

    registry.save()?;
    println!("{}", "Installation complete!".color_green().color_bold());

    Ok(())
}

fn cleanup_downloaded(downloads: &[DownloadedAddon]) {
    for d in downloads {
        let _ = cleanup_temp_dir(&d.temp_extract_dir);
        let _ = std::fs::remove_file(&d.temp_zip);
    }
}
