use crate::addon::{GameFlavor, InstalledAddon, ReleaseChannel};
use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use crate::sources::AddonSource;
use crate::sources::curseforge::CurseForgeSource;
use crate::utils::{
    find_dirs_claimed_by_modules, get_unmanaged_dirs, group_dirs_by_project_id, parse_toc_metadata,
};
use dialoguer::{Confirm, Select};
use std::collections::HashMap;
use tracing::{debug, warn};

pub async fn adopt(addon_folder: Option<&str>, all: bool, slug: Option<&str>) -> Result<()> {
    if addon_folder.is_none() && !all {
        return Err(WowctlError::Config(
            "Specify an addon folder name, or use --all to adopt all unmanaged addons".to_string(),
        ));
    }

    if all && slug.is_some() {
        return Err(WowctlError::Config(
            "--slug cannot be used with --all".to_string(),
        ));
    }

    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let api_key = config.get_api_key()?;
    let source = CurseForgeSource::new(api_key)?;
    let mut registry = Registry::load()?;

    if all {
        adopt_all(&addon_dir, &source, &mut registry).await
    } else {
        let folder = addon_folder.unwrap();
        adopt_single(folder, slug, &addon_dir, &source, &mut registry).await
    }
}

async fn adopt_single(
    folder: &str,
    slug_override: Option<&str>,
    addon_dir: &std::path::Path,
    source: &CurseForgeSource,
    registry: &mut Registry,
) -> Result<()> {
    let dir_path = addon_dir.join(folder);
    if !dir_path.is_dir() {
        return Err(WowctlError::InvalidAddonDir(format!(
            "Folder '{}' does not exist in {}",
            folder,
            addon_dir.display()
        )));
    }

    if registry.find_by_directory(folder).is_some() {
        return Err(WowctlError::Config(format!(
            "'{folder}' is already managed by wowctl"
        )));
    }

    let addon_info = if let Some(slug) = slug_override {
        println!("Looking up {}...", slug.color_cyan());
        source.get_addon_by_slug(slug).await?
    } else {
        resolve_addon_from_folder(folder, addon_dir, source).await?
    };

    let version_info = source
        .get_latest_version(&addon_info.id, ReleaseChannel::Stable)
        .await?;

    let unmanaged = get_unmanaged_dirs(addon_dir, registry)?;
    let mut directories = vec![folder.to_string()];

    // Group by TOC project ID to find sibling dirs with the same CF project
    let (by_id, _) = group_dirs_by_project_id(addon_dir, &unmanaged, GameFlavor::Retail);
    if let Some(siblings) = by_id.get(&addon_info.id) {
        for sib in siblings {
            if sib != folder && !directories.contains(sib) {
                directories.push(sib.clone());
            }
        }
    }

    // Also claim child dirs listed in the CF file's modules (e.g., dirs without TOC files)
    if !version_info.modules.is_empty() {
        for module_dir in &version_info.modules {
            if unmanaged.contains(module_dir) && !directories.contains(module_dir) {
                debug!("Claiming child directory '{}' via CF modules", module_dir);
                directories.push(module_dir.clone());
            }
        }
    }

    println!();
    println!("{}", "Adopt summary:".color_bold());
    println!("  Addon:       {}", addon_info.name.color_cyan());
    println!("  Slug:        {}", addon_info.slug.color_dimmed());
    println!("  Version:     {}", version_info.version.color_green());
    println!("  Directories: {}", directories.join(", "));
    println!();

    let confirmed = Confirm::new()
        .with_prompt("Adopt this addon?")
        .default(true)
        .interact()
        .unwrap_or(false);

    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }

    let installed = InstalledAddon {
        name: addon_info.name.clone(),
        slug: addon_info.slug.clone(),
        version: version_info.version,
        source: addon_info.source,
        addon_id: addon_info.id,
        directories,
        is_dependency: false,
        required_by: vec![],
        installed_file_id: Some(version_info.file_id),
        display_name: Some(version_info.display_name),
        channel: None,
        ignored: None,
        game_versions: Some(version_info.game_versions),
        released_at: Some(version_info.released_at),
        auto_update: None,
    };

    registry.add(installed);
    registry.save()?;

    println!(
        "Adopted {} {}",
        addon_info.name.color_cyan(),
        "successfully.".color_green()
    );

    Ok(())
}

// TODO: Consider fingerprint-based scanning like WoWUp — compute MurmurHash2 (whitespace-
// stripped, seed 1) per addon folder, batch-send to POST /v1/fingerprints, and match against
// file.modules[].fingerprint. This would identify the exact installed version (not just the
// addon) and work even when TOC files lack X-Curse-Project-ID. See WoWUp's scan() in
// curse-addon-provider.ts for reference.

/// Tries to identify the CurseForge addon from .toc metadata, falling back to name search.
async fn resolve_addon_from_folder(
    folder: &str,
    addon_dir: &std::path::Path,
    source: &CurseForgeSource,
) -> Result<crate::addon::AddonInfo> {
    if let Some(meta) = parse_toc_metadata(addon_dir, folder, GameFlavor::Retail) {
        if let Some(ref project_id) = meta.curse_project_id {
            println!(
                "Found CurseForge project ID {} in .toc for {}",
                project_id.color_green(),
                folder.color_cyan()
            );
            match source.get_addon_info_by_id(project_id).await {
                Ok(info) => return Ok(info),
                Err(e) => {
                    warn!("Failed to look up project ID {}: {}", project_id, e);
                }
            }
        }

        let search_term = meta.title.as_deref().unwrap_or(folder);
        return search_and_select(search_term, folder, source).await;
    }

    search_and_select(folder, folder, source).await
}

/// Searches CurseForge and lets the user pick from the results.
async fn search_and_select(
    query: &str,
    folder: &str,
    source: &CurseForgeSource,
) -> Result<crate::addon::AddonInfo> {
    println!("Searching CurseForge for '{}'...", query.color_cyan());
    let result = source.search(query, None).await?;

    if result.addons.is_empty() {
        return Err(WowctlError::AddonNotFound(format!(
            "No CurseForge results for '{folder}'. Use --slug to specify manually."
        )));
    }

    let display_items: Vec<String> = result
        .addons
        .iter()
        .take(10)
        .map(|r| format!("{} ({})", r.name, r.slug))
        .collect();

    let selection = Select::new()
        .with_prompt(format!("Select the addon for '{folder}'"))
        .items(&display_items)
        .default(0)
        .interact()
        .map_err(|e| WowctlError::Config(format!("Selection failed: {e}")))?;

    Ok(result
        .addons
        .into_iter()
        .nth(selection)
        .expect("selection index is in bounds"))
}

async fn adopt_all(
    addon_dir: &std::path::Path,
    source: &CurseForgeSource,
    registry: &mut Registry,
) -> Result<()> {
    let unmanaged = get_unmanaged_dirs(addon_dir, registry)?;

    if unmanaged.is_empty() {
        println!("{}", "No unmanaged addons found.".color_green());
        return Ok(());
    }

    println!(
        "Found {} unmanaged director{}.",
        unmanaged.len().to_string().color_cyan(),
        if unmanaged.len() == 1 { "y" } else { "ies" }
    );
    println!("Scanning .toc files for CurseForge project IDs...");
    println!();

    let (by_id, no_id) = group_dirs_by_project_id(addon_dir, &unmanaged, GameFlavor::Retail);

    // Resolve grouped addons by project ID
    let mut proposals: Vec<AdoptProposal> = Vec::new();
    let mut failed_dirs: Vec<String> = Vec::new();

    for (project_id, dirs) in &by_id {
        debug!("Looking up project ID {} for dirs: {:?}", project_id, dirs);
        match source.get_addon_info_by_id(project_id).await {
            Ok(addon_info) => {
                let (version, file_id, display_name, game_versions, released_at, modules) =
                    match source
                        .get_latest_version(&addon_info.id, ReleaseChannel::Stable)
                        .await
                    {
                        Ok(v) => (
                            v.version,
                            Some(v.file_id),
                            Some(v.display_name),
                            Some(v.game_versions),
                            Some(v.released_at),
                            v.modules,
                        ),
                        Err(e) => {
                            warn!("Could not get version for {}: {}", addon_info.name, e);
                            ("unknown".to_string(), None, None, None, None, Vec::new())
                        }
                    };
                proposals.push(AdoptProposal {
                    addon_info,
                    directories: dirs.clone(),
                    version,
                    file_id,
                    display_name,
                    game_versions,
                    released_at,
                    modules,
                });
            }
            Err(e) => {
                warn!("Failed to look up project ID {}: {}", project_id, e);
                failed_dirs.extend(dirs.clone());
            }
        }
    }

    // Try name-based search for dirs without project IDs
    let mut search_dirs: Vec<String> = no_id;
    search_dirs.extend(failed_dirs);

    // Deduplicate: skip dirs already covered by a proposal
    let already_covered: std::collections::HashSet<&str> = proposals
        .iter()
        .flat_map(|p| p.directories.iter().map(|d| d.as_str()))
        .collect();
    search_dirs.retain(|d| !already_covered.contains(d.as_str()));

    let mut unresolved = Vec::new();
    // Group remaining dirs that might belong to the same addon via name-search
    let mut searched_cache: HashMap<String, Option<crate::addon::AddonInfo>> = HashMap::new();

    for dir_name in &search_dirs {
        let search_term = parse_toc_metadata(addon_dir, dir_name, GameFlavor::Retail)
            .and_then(|m| m.title)
            .unwrap_or_else(|| dir_name.clone());

        if searched_cache.contains_key(&search_term) {
            continue;
        }

        debug!("Searching CurseForge for '{}'", search_term);
        match source.search(&search_term, None).await {
            Ok(result) if !result.addons.is_empty() => {
                let best = result.addons.into_iter().next().unwrap();
                let (version, file_id, display_name, game_versions, released_at, modules) =
                    match source
                        .get_latest_version(&best.id, ReleaseChannel::Stable)
                        .await
                    {
                        Ok(v) => (
                            v.version,
                            Some(v.file_id),
                            Some(v.display_name),
                            Some(v.game_versions),
                            Some(v.released_at),
                            v.modules,
                        ),
                        Err(_) => ("unknown".to_string(), None, None, None, None, Vec::new()),
                    };
                searched_cache.insert(search_term, Some(best.clone()));
                proposals.push(AdoptProposal {
                    addon_info: best,
                    directories: vec![dir_name.clone()],
                    version,
                    file_id,
                    display_name,
                    game_versions,
                    released_at,
                    modules,
                });
            }
            _ => {
                searched_cache.insert(search_term, None);
                unresolved.push(dir_name.clone());
            }
        }
    }

    if proposals.is_empty() {
        println!(
            "{}",
            "Could not match any unmanaged addons to CurseForge.".color_yellow()
        );
        if !unresolved.is_empty() {
            println!();
            println!("Unresolved directories:");
            for d in &unresolved {
                println!("  {}", d.color_dimmed());
            }
            println!();
            println!(
                "Use {} to adopt these manually.",
                "wowctl adopt <folder> --slug <slug>".color_cyan()
            );
        }
        return Ok(());
    }

    // Merge proposals that share the same addon ID
    let mut merged: HashMap<String, AdoptProposal> = HashMap::new();
    for proposal in proposals {
        let entry = merged
            .entry(proposal.addon_info.id.clone())
            .or_insert(AdoptProposal {
                addon_info: proposal.addon_info.clone(),
                directories: Vec::new(),
                version: proposal.version.clone(),
                file_id: proposal.file_id,
                display_name: proposal.display_name.clone(),
                game_versions: proposal.game_versions.clone(),
                released_at: proposal.released_at.clone(),
                modules: proposal.modules.clone(),
            });
        for dir in &proposal.directories {
            if !entry.directories.contains(dir) {
                entry.directories.push(dir.clone());
            }
        }
        for m in &proposal.modules {
            if !entry.modules.contains(m) {
                entry.modules.push(m.clone());
            }
        }
    }
    let mut proposals: Vec<AdoptProposal> = merged.into_values().collect();
    proposals.sort_by(|a, b| a.addon_info.name.cmp(&b.addon_info.name));

    // Claim unresolved dirs that appear in any proposal's CF modules list
    {
        let module_sets: Vec<(&str, &[String])> = proposals
            .iter()
            .map(|p| (p.addon_info.slug.as_str(), p.modules.as_slice()))
            .collect();
        let claimed = find_dirs_claimed_by_modules(&unresolved, &module_sets);
        // Convert to owned strings so we can drop the immutable borrow on proposals
        let claimed_owned: Vec<(String, String)> = claimed
            .into_iter()
            .map(|(d, s)| (d, s.to_string()))
            .collect();
        let claimed_dirs: std::collections::HashSet<&str> =
            claimed_owned.iter().map(|(d, _)| d.as_str()).collect();
        for (dir, parent_slug) in &claimed_owned {
            debug!(
                "Claiming child directory '{}' via modules of '{}'",
                dir, parent_slug
            );
            if let Some(proposal) = proposals
                .iter_mut()
                .find(|p| p.addon_info.slug == *parent_slug)
                && !proposal.directories.contains(dir)
            {
                proposal.directories.push(dir.clone());
            }
        }
        unresolved.retain(|d| !claimed_dirs.contains(d.as_str()));
    }

    // Present summary
    println!("{}", "Proposed adoptions:".color_bold());
    for (i, proposal) in proposals.iter().enumerate() {
        println!(
            "  {}. {} v{} [{}]",
            i + 1,
            proposal.addon_info.name.color_cyan(),
            proposal.version.color_green(),
            proposal.directories.join(", ").color_dimmed()
        );
    }

    if !unresolved.is_empty() {
        println!();
        println!(
            "{} unresolved director{} (use {} to adopt manually):",
            unresolved.len(),
            if unresolved.len() == 1 { "y" } else { "ies" },
            "wowctl adopt <folder> --slug <slug>".color_cyan()
        );
        for d in &unresolved {
            println!("  {}", d.color_yellow());
        }
    }

    println!();
    let confirmed = Confirm::new()
        .with_prompt(format!("Adopt {} addon(s)?", proposals.len()))
        .default(true)
        .interact()
        .unwrap_or(false);

    if !confirmed {
        println!("Cancelled.");
        return Ok(());
    }

    let mut adopted_count = 0;
    for proposal in proposals {
        let installed = InstalledAddon {
            name: proposal.addon_info.name.clone(),
            slug: proposal.addon_info.slug.clone(),
            version: proposal.version,
            source: proposal.addon_info.source,
            addon_id: proposal.addon_info.id,
            directories: proposal.directories,
            is_dependency: false,
            required_by: vec![],
            installed_file_id: proposal.file_id,
            display_name: proposal.display_name,
            channel: None,
            ignored: None,
            game_versions: proposal.game_versions,
            released_at: proposal.released_at,
            auto_update: None,
        };

        println!("  Adopting {}...", installed.name.color_cyan());
        registry.add(installed);
        adopted_count += 1;
    }

    registry.save()?;
    println!(
        "{}",
        format!("Adopted {adopted_count} addon(s) successfully.")
            .color_green()
            .color_bold()
    );

    Ok(())
}

struct AdoptProposal {
    addon_info: crate::addon::AddonInfo,
    directories: Vec<String>,
    version: String,
    file_id: Option<u32>,
    display_name: Option<String>,
    game_versions: Option<Vec<String>>,
    released_at: Option<String>,
    /// Canonical module directory names from the CurseForge file, used to detect child dirs.
    modules: Vec<String>,
}
