use crate::addon::{AddonInfo, InstalledAddon, ReleaseChannel, VersionInfo};
use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use crate::sources::{AddonSource, SourceKind, build_source, parse_addon_spec};
use crate::utils::{
    check_directory_conflicts, check_disk_space, cleanup_temp_dir, extract_zip_to_temp,
    move_addon_dirs,
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
    /// Folders this addon will own in the Registry (CurseForge modules when
    /// present, otherwise the zip's top-level directories).
    directories: Vec<String>,
}

/// The ownership-relevant view of one addon in an install plan.
struct PlannedDirs<'a> {
    name: &'a str,
    is_dependency: bool,
    directories: &'a [String],
}

/// Decides folder ownership for a whole install plan before any file moves.
///
/// Returns, per plan entry, `None` to install it or `Some(provider)` to skip a
/// dependency whose folders are all shipped by another addon — either one
/// already in the Registry or a strictly larger package in this same plan
/// (e.g. Altoholic bundles DataStore). Any other overlap is an error, raised
/// here so a failed install leaves nothing half-written on disk.
fn plan_directory_ownership(
    registry: &Registry,
    plan: &[PlannedDirs],
) -> Result<Vec<Option<String>>> {
    let is_superset = |outer: &[String], inner: &[String]| {
        !inner.is_empty() && inner.iter().all(|d| outer.contains(d))
    };

    let decisions: Vec<Option<String>> = plan
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            if !entry.is_dependency {
                return None;
            }
            if let Some(owner) = registry
                .addons
                .values()
                .find(|a| is_superset(&a.directories, entry.directories))
            {
                return Some(owner.name.clone());
            }
            plan.iter()
                .enumerate()
                .find(|(j, other)| {
                    *j != i
                        && other.directories.len() > entry.directories.len()
                        && is_superset(other.directories, entry.directories)
                })
                .map(|(_, other)| other.name.to_string())
        })
        .collect();

    let kept: Vec<&PlannedDirs> = plan
        .iter()
        .zip(&decisions)
        .filter(|(_, d)| d.is_none())
        .map(|(p, _)| p)
        .collect();

    for (i, entry) in kept.iter().enumerate() {
        check_directory_conflicts(registry, entry.directories, None)?;
        for other in &kept[i + 1..] {
            if let Some(dir) = entry
                .directories
                .iter()
                .find(|d| other.directories.contains(d))
            {
                return Err(WowctlError::Source(format!(
                    "Directory '{dir}' is shipped by both '{}' and '{}'",
                    entry.name, other.name
                )));
            }
        }
    }

    Ok(decisions)
}

/// Guards the Registry's one-Slug-one-Source invariant on install.
/// Ok(Some(existing)) = already installed from this same Source;
/// Err = installed from a different Source (user must remove first);
/// Ok(None) = not installed.
fn check_source_collision(
    registry: &Registry,
    slug: &str,
    kind: SourceKind,
) -> Result<Option<InstalledAddon>> {
    match registry.get(slug) {
        None => Ok(None),
        Some(existing) if existing.source == kind.as_str() => Ok(Some(existing.clone())),
        Some(existing) => Err(WowctlError::Source(format!(
            "'{slug}' is already installed from {}. An addon can only have one \
             update origin — run 'wowctl remove {slug}' first, then install it from {}.",
            existing.source,
            kind.as_str()
        ))),
    }
}

pub async fn install(addon: &str, channel_override: Option<ReleaseChannel>) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let channel = config.resolve_channel(channel_override);

    let (source_kind, slug) = parse_addon_spec(addon)?;

    let mut registry = Registry::load()?;

    if let Some(existing) = check_source_collision(&registry, &slug, source_kind)? {
        println!(
            "{} is already installed (version {})",
            existing.name.color_cyan(),
            existing.version.color_green()
        );
        return Ok(());
    }

    // Errors here include the missing-Wago-key guidance from build_source.
    let source = Arc::new(build_source(source_kind, &config)?);

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

            let directories = if version_info.modules.is_empty() {
                extracted_dirs.clone()
            } else {
                debug!(
                    "Using CurseForge modules for directory list: {:?} (zip had: {:?})",
                    version_info.modules, extracted_dirs
                );
                version_info.modules.clone()
            };

            Ok::<_, WowctlError>(DownloadedAddon {
                addon_info: addon_to_install,
                is_dependency: is_dep,
                version_info,
                temp_zip,
                temp_extract_dir,
                extracted_dirs,
                directories,
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

    let plan: Vec<PlannedDirs> = downloads
        .iter()
        .map(|d| PlannedDirs {
            name: &d.addon_info.name,
            is_dependency: d.is_dependency,
            directories: &d.directories,
        })
        .collect();
    let decisions = match plan_directory_ownership(&registry, &plan) {
        Ok(decisions) => decisions,
        Err(e) => {
            cleanup_downloaded(&downloads);
            return Err(e);
        }
    };

    // Sequential file move + registry update phase
    for (downloaded, provider) in downloads.into_iter().zip(decisions) {
        if let Some(provider) = provider {
            println!(
                "  Skipping dependency: {} (bundled with {})",
                downloaded.addon_info.name.color_cyan(),
                provider.color_cyan()
            );
            cleanup_downloaded(std::slice::from_ref(&downloaded));
            continue;
        }

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
            directories: downloaded.directories,
            is_dependency: downloaded.is_dependency,
            required_by: if downloaded.is_dependency {
                vec![slug.clone()]
            } else {
                vec![]
            },
            installed_file_id: downloaded.version_info.file_id,
            display_name: Some(downloaded.version_info.display_name),
            channel: addon_channel,
            ignored: None,
            game_versions: Some(downloaded.version_info.game_versions),
            released_at: Some(downloaded.version_info.released_at),
            auto_update: None,
            external_release_id: downloaded.version_info.external_release_id,
        };

        registry.add(installed);
        // Save per addon so a later failure never leaves untracked folders.
        registry.save()?;

        println!("{}", "done.".color_green());
    }

    println!("{}", "Installation complete!".color_green().color_bold());

    Ok(())
}

fn cleanup_downloaded(downloads: &[DownloadedAddon]) {
    for d in downloads {
        let _ = cleanup_temp_dir(&d.temp_extract_dir);
        let _ = std::fs::remove_file(&d.temp_zip);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceKind;

    fn registry_with(slug: &str, source: &str) -> Registry {
        let mut registry = Registry::default();
        registry.add(InstalledAddon {
            name: slug.to_string(),
            slug: slug.to_string(),
            version: "1.0.0".to_string(),
            source: source.to_string(),
            addon_id: "1".to_string(),
            directories: vec![],
            is_dependency: false,
            required_by: vec![],
            installed_file_id: None,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: None,
            auto_update: None,
            external_release_id: None,
        });
        registry
    }

    #[test]
    fn not_installed_returns_none() {
        let registry = Registry::default();
        let result = check_source_collision(&registry, "classcodex", SourceKind::Wago).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn same_source_returns_existing() {
        let registry = registry_with("classcodex", "wago");
        let existing = check_source_collision(&registry, "classcodex", SourceKind::Wago)
            .unwrap()
            .unwrap();
        assert_eq!(existing.slug, "classcodex");
    }

    #[test]
    fn different_source_is_hard_error() {
        let registry = registry_with("classcodex", "curseforge");
        let err = check_source_collision(&registry, "classcodex", SourceKind::Wago).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("curseforge"), "got: {msg}");
        assert!(msg.contains("wowctl remove classcodex"), "got: {msg}");
    }

    fn dirs(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn dependency_bundled_by_larger_plan_entry_is_skipped() {
        let registry = Registry::default();
        let target = dirs(&["WarboundGearRouter"]);
        let datastore = dirs(&["DataStore"]);
        let inventory = dirs(&["DataStore_Inventory"]);
        let altoholic = dirs(&["Altoholic", "DataStore", "DataStore_Inventory"]);
        let plan = [
            PlannedDirs {
                name: "Warbound",
                is_dependency: false,
                directories: &target,
            },
            PlannedDirs {
                name: "DataStore",
                is_dependency: true,
                directories: &datastore,
            },
            PlannedDirs {
                name: "DataStore_Inventory",
                is_dependency: true,
                directories: &inventory,
            },
            PlannedDirs {
                name: "Altoholic",
                is_dependency: true,
                directories: &altoholic,
            },
        ];
        let decisions = plan_directory_ownership(&registry, &plan).unwrap();
        assert_eq!(
            decisions,
            vec![
                None,
                Some("Altoholic".into()),
                Some("Altoholic".into()),
                None
            ]
        );
    }

    #[test]
    fn dependency_already_provided_by_installed_addon_is_skipped() {
        let mut registry = registry_with("altoholic", "curseforge");
        registry.addons.get_mut("altoholic").unwrap().directories =
            dirs(&["Altoholic", "DataStore"]);
        let datastore = dirs(&["DataStore"]);
        let plan = [PlannedDirs {
            name: "DataStore",
            is_dependency: true,
            directories: &datastore,
        }];
        let decisions = plan_directory_ownership(&registry, &plan).unwrap();
        assert_eq!(decisions, vec![Some("altoholic".into())]);
    }

    #[test]
    fn partial_overlap_in_plan_is_error() {
        let registry = Registry::default();
        let a = dirs(&["A", "Shared"]);
        let b = dirs(&["B", "Shared"]);
        let plan = [
            PlannedDirs {
                name: "A",
                is_dependency: false,
                directories: &a,
            },
            PlannedDirs {
                name: "B",
                is_dependency: true,
                directories: &b,
            },
        ];
        let msg = plan_directory_ownership(&registry, &plan)
            .unwrap_err()
            .to_string();
        assert!(msg.contains("Shared"), "got: {msg}");
    }

    #[test]
    fn explicit_target_conflicting_with_registry_is_error() {
        let mut registry = registry_with("altoholic", "curseforge");
        registry.addons.get_mut("altoholic").unwrap().directories = dirs(&["DataStore"]);
        let datastore = dirs(&["DataStore"]);
        let plan = [PlannedDirs {
            name: "DataStore",
            is_dependency: false,
            directories: &datastore,
        }];
        assert!(plan_directory_ownership(&registry, &plan).is_err());
    }
}
