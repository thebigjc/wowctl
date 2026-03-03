//! Utility functions for addon management.

use crate::addon::GameFlavor;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

/// Extracts the addon slug from a CurseForge URL.
pub fn extract_slug_from_url(url: &str) -> Result<String> {
    if url.contains("curseforge.com/wow/addons/") {
        let parts: Vec<&str> = url.split('/').collect();
        if let Some(slug) = parts.last() {
            return Ok(slug.to_string());
        }
    }

    Err(WowctlError::Source(format!(
        "Invalid CurseForge URL: {}",
        url
    )))
}

/// Validates that a path exists and is a directory.
pub fn validate_addon_dir(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(WowctlError::InvalidAddonDir(format!(
            "Directory does not exist: {}",
            path.display()
        )));
    }

    if !path.is_dir() {
        return Err(WowctlError::InvalidAddonDir(format!(
            "Path is not a directory: {}",
            path.display()
        )));
    }

    Ok(())
}

/// Determines whether colored output should be used based on config and flags.
pub fn should_use_color(config_color: bool, no_color_flag: bool) -> bool {
    if no_color_flag {
        return false;
    }

    if std::env::var("NO_COLOR").is_ok() {
        return false;
    }

    config_color
}

/// Extracts a zip file and returns the list of top-level directories created.
pub fn extract_zip(zip_path: &Path, extract_to: &Path) -> Result<Vec<String>> {
    info!(
        "Extracting {} to {}",
        zip_path.display(),
        extract_to.display()
    );

    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)?;

    let mut extracted_dirs = std::collections::HashSet::new();

    for i in 0..archive.len() {
        let mut file = archive.by_index(i)?;
        let outpath = match file.enclosed_name() {
            Some(path) => extract_to.join(path),
            None => continue,
        };

        if file.name().ends_with('/') {
            debug!("Creating directory: {}", outpath.display());
            fs::create_dir_all(&outpath)?;
        } else {
            if let Some(parent) = outpath.parent()
                && !parent.exists()
            {
                fs::create_dir_all(parent)?;
            }

            let mut outfile = fs::File::create(&outpath)?;
            std::io::copy(&mut file, &mut outfile)?;
        }

        if let Some(enclosed) = file.enclosed_name()
            && let Some(std::path::Component::Normal(dir_name)) = enclosed.components().next()
            && let Some(dir_str) = dir_name.to_str()
        {
            extracted_dirs.insert(dir_str.to_string());
        }
    }

    let dirs: Vec<String> = extracted_dirs.into_iter().collect();
    debug!("Extracted {} top-level directories: {:?}", dirs.len(), dirs);

    Ok(dirs)
}

/// Extracts a zip file to a temporary directory and returns the temp path and directories.
pub fn extract_zip_to_temp(zip_path: &Path) -> Result<(PathBuf, Vec<String>)> {
    let temp_dir = std::env::temp_dir().join(format!("wowctl-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&temp_dir)?;

    let dirs = extract_zip(zip_path, &temp_dir)?;

    Ok((temp_dir, dirs))
}

/// Moves addon directories from a temporary location to the addon directory.
pub fn move_addon_dirs(temp_dir: &Path, addon_dir: &Path, directories: &[String]) -> Result<()> {
    info!("Moving addon directories to {}", addon_dir.display());

    for dir_name in directories {
        let src = temp_dir.join(dir_name);
        let dest = addon_dir.join(dir_name);

        if dest.exists() {
            debug!("Removing existing directory: {}", dest.display());
            fs::remove_dir_all(&dest)?;
        }

        debug!("Moving {} to {}", src.display(), dest.display());
        fs::rename(&src, &dest)?;
    }

    Ok(())
}

/// Cleans up a temporary directory.
pub fn cleanup_temp_dir(temp_dir: &Path) -> Result<()> {
    if temp_dir.exists() {
        debug!("Cleaning up temporary directory: {}", temp_dir.display());
        fs::remove_dir_all(temp_dir)?;
    }
    Ok(())
}

/// Checks if any of the directories are already owned by another addon.
pub fn check_directory_conflicts(
    registry: &crate::registry::Registry,
    directories: &[String],
    current_addon_slug: Option<&str>,
) -> Result<()> {
    for dir in directories {
        if let Some(existing_addon) = registry.find_by_directory(dir)
            && Some(existing_addon.slug.as_str()) != current_addon_slug
        {
            return Err(WowctlError::Source(format!(
                "Directory '{}' is already owned by addon '{}'",
                dir, existing_addon.name
            )));
        }
    }
    Ok(())
}

/// Metadata extracted from a WoW addon .toc file.
#[derive(Debug, Clone, Default)]
pub struct TocMetadata {
    pub curse_project_id: Option<String>,
    pub title: Option<String>,
    pub version: Option<String>,
    /// WoW client interface versions this addon supports (from `## Interface:`).
    /// Multi-version TOCs use comma-separated values, e.g. `## Interface: 110002, 40400`.
    pub interface_versions: Vec<String>,
    /// Required addon dependencies (from `## Dependencies:` and `## RequiredDeps:`).
    /// These are addon folder names that must be loaded before this addon.
    pub dependencies: Vec<String>,
    /// WoWInterface addon ID (from `## X-WoWI-ID:`).
    pub wowi_id: Option<String>,
    /// Wago addon ID (from `## X-Wago-ID:`).
    pub wago_id: Option<String>,
    /// Tukui project ID (from `## X-Tukui-ProjectID:`).
    pub tukui_id: Option<String>,
}

/// Selects the best `.toc` file from an addon directory for the given game flavor.
///
/// Preference order:
/// 1. `{FolderName}{flavor_suffix}.toc` (e.g., `MyAddon_Mainline.toc` for Retail)
/// 2. `{FolderName}.toc` (base TOC)
/// 3. Any `.toc` file (fallback for non-standard naming)
fn select_toc_file(dir_path: &Path, folder_name: &str, flavor: GameFlavor) -> Option<PathBuf> {
    let toc_files: Vec<PathBuf> = fs::read_dir(dir_path)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .and_then(OsStr::to_str)
                .is_some_and(|ext| ext.eq_ignore_ascii_case("toc"))
        })
        .collect();

    if toc_files.is_empty() {
        return None;
    }

    let flavor_name = format!("{}{}.toc", folder_name, flavor.toc_suffix());
    if let Some(p) = toc_files.iter().find(|p| {
        p.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n.eq_ignore_ascii_case(&flavor_name))
    }) {
        debug!("Selected flavor-specific TOC: {}", p.display());
        return Some(p.clone());
    }

    let base_name = format!("{}.toc", folder_name);
    if let Some(p) = toc_files.iter().find(|p| {
        p.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n.eq_ignore_ascii_case(&base_name))
    }) {
        debug!("Selected base TOC: {}", p.display());
        return Some(p.clone());
    }

    debug!(
        "Falling back to first available TOC: {}",
        toc_files[0].display()
    );
    Some(toc_files[0].clone())
}

/// Strips WoW UI escape codes from a string, keeping only visible text.
///
/// Handles: `|cAARRGGBB` (color start), `|r` (color reset), `|T...|t` (texture),
/// `|H...|h...|h` (hyperlink — keeps display text), `|n` (newline marker).
fn strip_wow_escape_codes(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0;

    while i < len {
        if bytes[i] == b'|' && i + 1 < len {
            match bytes[i + 1] {
                b'c' | b'C' => {
                    // |cAARRGGBB — skip pipe + 'c' + 8 hex digits (10 chars total)
                    let skip = 10.min(len - i);
                    i += skip;
                }
                b'r' | b'R' => {
                    i += 2;
                }
                b'T' | b't' => {
                    // |T...|t — texture reference; skip until closing |t
                    i += 2;
                    while i < len {
                        if bytes[i] == b'|'
                            && i + 1 < len
                            && (bytes[i + 1] == b't' || bytes[i + 1] == b'T')
                        {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                }
                b'H' => {
                    // |H...|h<display text>|h — skip link data, keep display text
                    i += 2;
                    // Skip until first |h (end of link metadata)
                    while i < len {
                        if bytes[i] == b'|'
                            && i + 1 < len
                            && (bytes[i + 1] == b'h' || bytes[i + 1] == b'H')
                        {
                            i += 2;
                            break;
                        }
                        i += 1;
                    }
                    // Now collect display text until closing |h
                    while i < len {
                        if bytes[i] == b'|'
                            && i + 1 < len
                            && (bytes[i + 1] == b'h' || bytes[i + 1] == b'H')
                        {
                            i += 2;
                            break;
                        }
                        result.push(bytes[i] as char);
                        i += 1;
                    }
                }
                b'n' => {
                    result.push(' ');
                    i += 2;
                }
                _ => {
                    result.push('|');
                    i += 1;
                }
            }
        } else {
            result.push(bytes[i] as char);
            i += 1;
        }
    }

    result
}

/// Parses a TOC file's content into metadata.
fn parse_toc_contents(contents: &str) -> TocMetadata {
    let mut meta = TocMetadata::default();

    for line in contents.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("##") {
            let rest = rest.trim();
            if let Some((key, value)) = rest.split_once(':') {
                let key = key.trim();
                let value = value.trim();
                match key {
                    "X-Curse-Project-ID" => meta.curse_project_id = Some(value.to_string()),
                    "Title" => {
                        let clean = strip_wow_escape_codes(value).trim().to_string();
                        meta.title = if clean.is_empty() { None } else { Some(clean) };
                    }
                    "Version" => meta.version = Some(value.to_string()),
                    "Interface" => {
                        meta.interface_versions = value
                            .split(',')
                            .map(|v| v.trim().to_string())
                            .filter(|v| !v.is_empty())
                            .collect();
                    }
                    "Dependencies" | "RequiredDeps" => {
                        for dep in value.split(',').map(|v| v.trim()).filter(|v| !v.is_empty()) {
                            if !meta.dependencies.contains(&dep.to_string()) {
                                meta.dependencies.push(dep.to_string());
                            }
                        }
                    }
                    "X-WoWI-ID" => {
                        if !value.is_empty() {
                            meta.wowi_id = Some(value.to_string());
                        }
                    }
                    "X-Wago-ID" => {
                        if !value.is_empty() {
                            meta.wago_id = Some(value.to_string());
                        }
                    }
                    "X-Tukui-ProjectID" => {
                        if !value.is_empty() {
                            meta.tukui_id = Some(value.to_string());
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    meta
}

/// Parses the best `.toc` file in an addon directory for metadata,
/// preferring the flavor-specific TOC for the given game version.
///
/// WoW .toc files use `## Key: Value` format for metadata fields.
pub fn parse_toc_metadata(
    addon_dir: &Path,
    folder_name: &str,
    flavor: GameFlavor,
) -> Option<TocMetadata> {
    let dir_path = addon_dir.join(folder_name);
    if !dir_path.is_dir() {
        return None;
    }

    let toc_path = select_toc_file(&dir_path, folder_name, flavor)?;
    let contents = fs::read_to_string(&toc_path).ok()?;
    let meta = parse_toc_contents(&contents);

    debug!(
        "Parsed {} for {}: project_id={:?}, title={:?}, version={:?}, interface={:?}, deps={:?}, wowi={:?}, wago={:?}, tukui={:?}",
        toc_path.file_name().unwrap_or_default().to_string_lossy(),
        folder_name,
        meta.curse_project_id,
        meta.title,
        meta.version,
        meta.interface_versions,
        meta.dependencies,
        meta.wowi_id,
        meta.wago_id,
        meta.tukui_id
    );
    Some(meta)
}

/// Returns the set of addon directory names not tracked by the registry.
pub fn get_unmanaged_dirs(addon_dir: &Path, registry: &Registry) -> Result<Vec<String>> {
    if !addon_dir.exists() {
        return Err(WowctlError::InvalidAddonDir(format!(
            "Addon directory does not exist: {}",
            addon_dir.display()
        )));
    }

    let mut all_dirs = HashSet::new();
    if let Ok(entries) = fs::read_dir(addon_dir) {
        for entry in entries.flatten() {
            if entry.path().is_dir()
                && let Some(name) = entry.file_name().to_str()
            {
                all_dirs.insert(name.to_string());
            }
        }
    }

    let mut managed_dirs = HashSet::new();
    for addon in registry.list_all() {
        for dir in &addon.directories {
            managed_dirs.insert(dir.clone());
        }
    }

    let mut unmanaged: Vec<String> = all_dirs.difference(&managed_dirs).cloned().collect();
    unmanaged.sort();
    Ok(unmanaged)
}

/// Identifies unmanaged directories that are child modules of known addons.
///
/// Given a list of unmanaged directory names and a set of (addon_slug, modules) pairs,
/// returns a map of `dir_name -> parent_addon_slug` for any dir that appears in
/// a module list but isn't otherwise tracked.
pub fn find_dirs_claimed_by_modules<'a>(
    unmanaged: &[String],
    module_sets: &[(&'a str, &[String])],
) -> HashMap<String, &'a str> {
    let mut claimed: HashMap<String, &str> = HashMap::new();
    for dir_name in unmanaged {
        for &(slug, modules) in module_sets {
            if modules.iter().any(|m| m == dir_name) {
                claimed.insert(dir_name.clone(), slug);
                break;
            }
        }
    }
    claimed
}

/// Checks whether a directory has a `.toc` file.
pub fn dir_has_toc(addon_dir: &Path, dir_name: &str) -> bool {
    let dir_path = addon_dir.join(dir_name);
    if !dir_path.is_dir() {
        return false;
    }
    std::fs::read_dir(&dir_path)
        .ok()
        .map(|entries| {
            entries.filter_map(|e| e.ok()).any(|e| {
                e.path()
                    .extension()
                    .and_then(std::ffi::OsStr::to_str)
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("toc"))
            })
        })
        .unwrap_or(false)
}

/// Groups unmanaged directories by their CurseForge project ID from .toc metadata.
/// Returns (grouped_by_id, dirs_without_id).
pub fn group_dirs_by_project_id(
    addon_dir: &Path,
    dirs: &[String],
    flavor: GameFlavor,
) -> (HashMap<String, Vec<String>>, Vec<String>) {
    let mut by_id: HashMap<String, Vec<String>> = HashMap::new();
    let mut no_id = Vec::new();

    for dir_name in dirs {
        if let Some(meta) = parse_toc_metadata(addon_dir, dir_name, flavor) {
            if let Some(ref pid) = meta.curse_project_id {
                by_id.entry(pid.clone()).or_default().push(dir_name.clone());
            } else {
                no_id.push(dir_name.clone());
            }
        } else {
            no_id.push(dir_name.clone());
        }
    }

    (by_id, no_id)
}

const BACKUP_SUFFIX: &str = "-wowctl-bak";

/// Backs up addon directories by renaming them with a `-wowctl-bak` suffix.
/// Returns the list of directories that were successfully backed up.
pub fn backup_addon_dirs(addon_dir: &Path, directories: &[String]) -> Result<Vec<String>> {
    let mut backed_up = Vec::new();

    for dir_name in directories {
        let original = addon_dir.join(dir_name);
        if !original.exists() {
            continue;
        }
        let backup = addon_dir.join(format!("{}{}", dir_name, BACKUP_SUFFIX));
        if backup.exists() {
            fs::remove_dir_all(&backup)?;
        }
        debug!("Backing up {} -> {}", original.display(), backup.display());
        fs::rename(&original, &backup)?;
        backed_up.push(dir_name.clone());
    }

    Ok(backed_up)
}

/// Restores addon directories from their `-wowctl-bak` backups.
/// Removes any partially-installed new directories first.
pub fn restore_addon_dirs(addon_dir: &Path, backed_up_dirs: &[String]) {
    for dir_name in backed_up_dirs {
        let original = addon_dir.join(dir_name);
        let backup = addon_dir.join(format!("{}{}", dir_name, BACKUP_SUFFIX));

        if original.exists()
            && let Err(e) = fs::remove_dir_all(&original)
        {
            warn!(
                "Failed to remove partial install {}: {}",
                original.display(),
                e
            );
        }

        if backup.exists() {
            if let Err(e) = fs::rename(&backup, &original) {
                warn!(
                    "Failed to restore backup {} -> {}: {}",
                    backup.display(),
                    original.display(),
                    e
                );
            } else {
                debug!("Restored {} from backup", dir_name);
            }
        }
    }
}

/// Removes `-wowctl-bak` backup directories after a successful update.
pub fn cleanup_backup_dirs(addon_dir: &Path, backed_up_dirs: &[String]) {
    for dir_name in backed_up_dirs {
        let backup = addon_dir.join(format!("{}{}", dir_name, BACKUP_SUFFIX));
        if backup.exists() {
            if let Err(e) = fs::remove_dir_all(&backup) {
                warn!("Failed to clean up backup {}: {}", backup.display(), e);
            } else {
                debug!("Cleaned up backup for {}", dir_name);
            }
        }
    }
}

/// Checks if there is sufficient disk space available for a download.
///
/// Returns Ok if there's enough space, or a warning if space is low.
/// `required_bytes` is the estimated space needed for the operation.
/// `path` is the directory where files will be written.
pub fn check_disk_space(path: &Path, required_bytes: u64) -> Result<()> {
    const SAFETY_MARGIN: u64 = 100 * 1024 * 1024;

    match fs2::free_space(path) {
        Ok(available) => {
            debug!(
                "Available disk space: {} bytes, required: {} bytes",
                available, required_bytes
            );

            let needed = required_bytes + SAFETY_MARGIN;
            if available < needed {
                return Err(WowctlError::Io(std::io::Error::other(format!(
                    "Insufficient disk space: {} MB available, {} MB required",
                    available / (1024 * 1024),
                    needed / (1024 * 1024)
                ))));
            }

            if available < needed * 2 {
                warn!(
                    "Low disk space: {} MB available, {} MB required",
                    available / (1024 * 1024),
                    needed / (1024 * 1024)
                );
            }

            Ok(())
        }
        Err(e) => {
            debug!("Could not check disk space: {}", e);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn create_dir_with_file(base: &Path, dir_name: &str, file_name: &str, content: &str) {
        let dir = base.join(dir_name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(file_name), content).unwrap();
    }

    #[test]
    fn backup_renames_dirs_with_suffix() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "MyAddon", "file.lua", "-- hello");
        create_dir_with_file(addon_dir, "MyAddon_Options", "opts.lua", "-- opts");

        let dirs = vec!["MyAddon".into(), "MyAddon_Options".into()];
        let backed_up = backup_addon_dirs(addon_dir, &dirs).unwrap();

        assert_eq!(backed_up.len(), 2);
        assert!(!addon_dir.join("MyAddon").exists());
        assert!(!addon_dir.join("MyAddon_Options").exists());
        assert!(addon_dir.join("MyAddon-wowctl-bak").exists());
        assert!(addon_dir.join("MyAddon_Options-wowctl-bak").exists());
        let content = fs::read_to_string(addon_dir.join("MyAddon-wowctl-bak/file.lua")).unwrap();
        assert_eq!(content, "-- hello");
    }

    #[test]
    fn backup_skips_nonexistent_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "Exists", "f.lua", "x");

        let dirs = vec!["Exists".into(), "DoesNotExist".into()];
        let backed_up = backup_addon_dirs(addon_dir, &dirs).unwrap();

        assert_eq!(backed_up, vec!["Exists".to_string()]);
    }

    #[test]
    fn restore_puts_backups_back() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "MyAddon-wowctl-bak", "file.lua", "-- original");

        let backed_up = vec!["MyAddon".to_string()];
        restore_addon_dirs(addon_dir, &backed_up);

        assert!(addon_dir.join("MyAddon").exists());
        assert!(!addon_dir.join("MyAddon-wowctl-bak").exists());
        let content = fs::read_to_string(addon_dir.join("MyAddon/file.lua")).unwrap();
        assert_eq!(content, "-- original");
    }

    #[test]
    fn restore_removes_partial_install_before_restoring() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "MyAddon", "new.lua", "-- partial new");
        create_dir_with_file(addon_dir, "MyAddon-wowctl-bak", "old.lua", "-- original");

        let backed_up = vec!["MyAddon".to_string()];
        restore_addon_dirs(addon_dir, &backed_up);

        assert!(addon_dir.join("MyAddon/old.lua").exists());
        assert!(!addon_dir.join("MyAddon/new.lua").exists());
    }

    #[test]
    fn cleanup_removes_backups() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "MyAddon-wowctl-bak", "file.lua", "-- old");
        create_dir_with_file(addon_dir, "MyAddon", "file.lua", "-- new");

        let backed_up = vec!["MyAddon".to_string()];
        cleanup_backup_dirs(addon_dir, &backed_up);

        assert!(!addon_dir.join("MyAddon-wowctl-bak").exists());
        assert!(addon_dir.join("MyAddon").exists());
    }

    #[test]
    fn full_backup_restore_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        create_dir_with_file(addon_dir, "Addon1", "init.lua", "v1");
        create_dir_with_file(addon_dir, "Addon2", "core.lua", "v1");

        let dirs = vec!["Addon1".into(), "Addon2".into()];
        let backed_up = backup_addon_dirs(addon_dir, &dirs).unwrap();

        assert!(!addon_dir.join("Addon1").exists());
        assert!(!addon_dir.join("Addon2").exists());

        restore_addon_dirs(addon_dir, &backed_up);

        assert!(addon_dir.join("Addon1/init.lua").exists());
        assert!(addon_dir.join("Addon2/core.lua").exists());
        assert_eq!(
            fs::read_to_string(addon_dir.join("Addon1/init.lua")).unwrap(),
            "v1"
        );
    }

    fn write_toc(base: &Path, folder: &str, toc_name: &str, content: &str) {
        let dir = base.join(folder);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join(toc_name), content).unwrap();
    }

    #[test]
    fn toc_prefers_mainline_for_retail() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let base_toc = "## Title: MyAddon Base\n## Version: 1.0\n## X-Curse-Project-ID: 100\n";
        let mainline_toc =
            "## Title: MyAddon Mainline\n## Version: 2.0\n## X-Curse-Project-ID: 100\n";
        let classic_toc =
            "## Title: MyAddon Classic\n## Version: 0.5\n## X-Curse-Project-ID: 100\n";

        write_toc(addon_dir, "MyAddon", "MyAddon.toc", base_toc);
        write_toc(addon_dir, "MyAddon", "MyAddon_Mainline.toc", mainline_toc);
        write_toc(addon_dir, "MyAddon", "MyAddon_Classic.toc", classic_toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("MyAddon Mainline"));
        assert_eq!(meta.version.as_deref(), Some("2.0"));
    }

    #[test]
    fn toc_prefers_classic_for_classic_flavor() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let base_toc = "## Title: MyAddon Base\n## Version: 1.0\n";
        let mainline_toc = "## Title: MyAddon Mainline\n## Version: 2.0\n";
        let classic_toc = "## Title: MyAddon Classic\n## Version: 0.5\n";

        write_toc(addon_dir, "MyAddon", "MyAddon.toc", base_toc);
        write_toc(addon_dir, "MyAddon", "MyAddon_Mainline.toc", mainline_toc);
        write_toc(addon_dir, "MyAddon", "MyAddon_Classic.toc", classic_toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Classic).unwrap();
        assert_eq!(meta.title.as_deref(), Some("MyAddon Classic"));
        assert_eq!(meta.version.as_deref(), Some("0.5"));
    }

    #[test]
    fn toc_falls_back_to_base_when_no_flavor_specific() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let base_toc = "## Title: MyAddon\n## Version: 1.0\n## X-Curse-Project-ID: 42\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", base_toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("MyAddon"));
        assert_eq!(meta.version.as_deref(), Some("1.0"));
        assert_eq!(meta.curse_project_id.as_deref(), Some("42"));
    }

    #[test]
    fn toc_falls_back_to_any_toc_when_no_matching_name() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: Oddly Named\n## Version: 3.0\n";
        write_toc(addon_dir, "MyAddon", "SomethingElse.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Oddly Named"));
    }

    #[test]
    fn toc_returns_none_for_empty_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();
        fs::create_dir_all(addon_dir.join("EmptyAddon")).unwrap();

        let meta = parse_toc_metadata(addon_dir, "EmptyAddon", GameFlavor::Retail);
        assert!(meta.is_none());
    }

    #[test]
    fn toc_returns_none_for_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let meta = parse_toc_metadata(addon_dir, "DoesNotExist", GameFlavor::Retail);
        assert!(meta.is_none());
    }

    #[test]
    fn toc_selection_is_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: Details\n## Version: 1.0\n";
        write_toc(addon_dir, "Details", "Details_MAINLINE.TOC", toc);

        let meta = parse_toc_metadata(addon_dir, "Details", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Details"));
    }

    #[test]
    fn toc_parses_single_interface_version() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Interface: 110002\n## Title: MyAddon\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.interface_versions, vec!["110002"]);
    }

    #[test]
    fn toc_parses_multiple_interface_versions() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Interface: 110002, 40400, 11503\n## Title: MultiVersion\n";
        write_toc(addon_dir, "MultiVersion", "MultiVersion.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MultiVersion", GameFlavor::Retail).unwrap();
        assert_eq!(meta.interface_versions, vec!["110002", "40400", "11503"]);
    }

    #[test]
    fn toc_interface_empty_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: NoInterface\n## Version: 1.0\n";
        write_toc(addon_dir, "NoInterface", "NoInterface.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "NoInterface", GameFlavor::Retail).unwrap();
        assert!(meta.interface_versions.is_empty());
    }

    #[test]
    fn toc_interface_handles_extra_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Interface:  110002 ,  40400 ,  11503 \n## Title: Spacey\n";
        write_toc(addon_dir, "Spacey", "Spacey.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "Spacey", GameFlavor::Retail).unwrap();
        assert_eq!(meta.interface_versions, vec!["110002", "40400", "11503"]);
    }

    #[test]
    fn toc_parses_dependencies() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MyAddon\n## Dependencies: Ace3, LibSharedMedia-3.0\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.dependencies, vec!["Ace3", "LibSharedMedia-3.0"]);
    }

    #[test]
    fn toc_parses_required_deps() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MyAddon\n## RequiredDeps: LibStub, CallbackHandler-1.0\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.dependencies, vec!["LibStub", "CallbackHandler-1.0"]);
    }

    #[test]
    fn toc_merges_dependencies_and_required_deps() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MyAddon\n## Dependencies: Ace3, LibStub\n## RequiredDeps: LibStub, LibDBIcon-1.0\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.dependencies, vec!["Ace3", "LibStub", "LibDBIcon-1.0"]);
    }

    #[test]
    fn toc_dependencies_empty_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: NoDeps\n## Version: 1.0\n";
        write_toc(addon_dir, "NoDeps", "NoDeps.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "NoDeps", GameFlavor::Retail).unwrap();
        assert!(meta.dependencies.is_empty());
    }

    #[test]
    fn toc_dependencies_handles_whitespace() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: Spacey\n## Dependencies:  Ace3 ,  LibStub ,  \n";
        write_toc(addon_dir, "Spacey", "Spacey.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "Spacey", GameFlavor::Retail).unwrap();
        assert_eq!(meta.dependencies, vec!["Ace3", "LibStub"]);
    }

    #[test]
    fn toc_single_dependency() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: Simple\n## Dependencies: Blizzard_CombatLog\n";
        write_toc(addon_dir, "Simple", "Simple.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "Simple", GameFlavor::Retail).unwrap();
        assert_eq!(meta.dependencies, vec!["Blizzard_CombatLog"]);
    }

    // --- TOC source ID extraction tests ---

    #[test]
    fn toc_parses_wowi_id() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MyAddon\n## X-WoWI-ID: 12345\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.wowi_id.as_deref(), Some("12345"));
    }

    #[test]
    fn toc_parses_wago_id() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MyAddon\n## X-Wago-ID: aB3cD4eF\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.wago_id.as_deref(), Some("aB3cD4eF"));
    }

    #[test]
    fn toc_parses_tukui_id() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: ElvUI\n## X-Tukui-ProjectID: 1\n";
        write_toc(addon_dir, "ElvUI", "ElvUI.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "ElvUI", GameFlavor::Retail).unwrap();
        assert_eq!(meta.tukui_id.as_deref(), Some("1"));
    }

    #[test]
    fn toc_parses_all_source_ids_together() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: MultiSource\n## X-Curse-Project-ID: 99\n## X-WoWI-ID: 555\n## X-Wago-ID: xYz123\n## X-Tukui-ProjectID: 42\n";
        write_toc(addon_dir, "MultiSource", "MultiSource.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MultiSource", GameFlavor::Retail).unwrap();
        assert_eq!(meta.curse_project_id.as_deref(), Some("99"));
        assert_eq!(meta.wowi_id.as_deref(), Some("555"));
        assert_eq!(meta.wago_id.as_deref(), Some("xYz123"));
        assert_eq!(meta.tukui_id.as_deref(), Some("42"));
    }

    #[test]
    fn toc_source_ids_none_when_absent() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: NoSourceIDs\n## Version: 1.0\n";
        write_toc(addon_dir, "NoSourceIDs", "NoSourceIDs.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "NoSourceIDs", GameFlavor::Retail).unwrap();
        assert!(meta.wowi_id.is_none());
        assert!(meta.wago_id.is_none());
        assert!(meta.tukui_id.is_none());
    }

    // --- strip_wow_escape_codes tests ---

    #[test]
    fn strip_color_codes() {
        assert_eq!(
            strip_wow_escape_codes("|cFF00FF00Green Text|r"),
            "Green Text"
        );
    }

    #[test]
    fn strip_color_codes_preserves_surrounding_text() {
        assert_eq!(
            strip_wow_escape_codes("Before |cFFFF0000Red|r After"),
            "Before Red After"
        );
    }

    #[test]
    fn strip_nested_color_codes() {
        assert_eq!(
            strip_wow_escape_codes("|cFF0000FFBlue |cFFFF0000Red|r|r rest"),
            "Blue Red rest"
        );
    }

    #[test]
    fn strip_texture_codes() {
        assert_eq!(
            strip_wow_escape_codes("Icon |TInterface\\Icons\\Spell_Nature_Heal:16|t Healing"),
            "Icon  Healing"
        );
    }

    #[test]
    fn strip_hyperlink_keeps_display_text() {
        assert_eq!(
            strip_wow_escape_codes("|Hitem:12345:0|hSome Item|h"),
            "Some Item"
        );
    }

    #[test]
    fn strip_newline_marker() {
        assert_eq!(strip_wow_escape_codes("Line1|nLine2"), "Line1 Line2");
    }

    #[test]
    fn strip_plain_text_unchanged() {
        assert_eq!(
            strip_wow_escape_codes("Just A Normal Title"),
            "Just A Normal Title"
        );
    }

    #[test]
    fn strip_complex_title_with_multiple_codes() {
        assert_eq!(
            strip_wow_escape_codes("|cFFFFFFFFDetails!|r |cFF00FF00v2.0|r"),
            "Details! v2.0"
        );
    }

    #[test]
    fn strip_preserves_literal_pipe_without_known_code() {
        assert_eq!(strip_wow_escape_codes("Foo | Bar"), "Foo | Bar");
    }

    #[test]
    fn toc_title_strips_color_codes() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: |cFF00FF00Details!|r |cFFAABBCCEnhanced|r\n## Version: 2.0\n";
        write_toc(addon_dir, "Details", "Details.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "Details", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("Details! Enhanced"));
    }

    #[test]
    fn toc_title_strips_texture_and_color() {
        let tmp = tempfile::tempdir().unwrap();
        let addon_dir = tmp.path();

        let toc = "## Title: |TInterface\\AddOns\\MyAddon\\icon:16|t |cFF00FFFFMyAddon|r\n";
        write_toc(addon_dir, "MyAddon", "MyAddon.toc", toc);

        let meta = parse_toc_metadata(addon_dir, "MyAddon", GameFlavor::Retail).unwrap();
        assert_eq!(meta.title.as_deref(), Some("MyAddon"));
    }

    // --- find_dirs_claimed_by_modules tests ---

    #[test]
    fn find_claimed_dirs_matches_child_modules() {
        let unmanaged = vec![
            "WeakAurasOptions".to_string(),
            "WeakAurasTemplates".to_string(),
            "SomeUnrelated".to_string(),
        ];
        let modules = vec![
            "WeakAuras".to_string(),
            "WeakAurasOptions".to_string(),
            "WeakAurasTemplates".to_string(),
        ];
        let sets: Vec<(&str, &[String])> = vec![("weakauras-2", modules.as_slice())];
        let claimed = find_dirs_claimed_by_modules(&unmanaged, &sets);

        assert_eq!(claimed.len(), 2);
        assert_eq!(claimed["WeakAurasOptions"], "weakauras-2");
        assert_eq!(claimed["WeakAurasTemplates"], "weakauras-2");
        assert!(!claimed.contains_key("SomeUnrelated"));
    }

    #[test]
    fn find_claimed_dirs_empty_when_no_match() {
        let unmanaged = vec!["RandomDir".to_string()];
        let modules = vec!["WeakAuras".to_string(), "WeakAurasOptions".to_string()];
        let sets: Vec<(&str, &[String])> = vec![("weakauras-2", modules.as_slice())];
        let claimed = find_dirs_claimed_by_modules(&unmanaged, &sets);

        assert!(claimed.is_empty());
    }

    #[test]
    fn find_claimed_dirs_multiple_addons() {
        let unmanaged = vec![
            "Details_DataStorage".to_string(),
            "WeakAurasOptions".to_string(),
        ];
        let details_mods = vec!["Details".to_string(), "Details_DataStorage".to_string()];
        let wa_mods = vec!["WeakAuras".to_string(), "WeakAurasOptions".to_string()];
        let sets: Vec<(&str, &[String])> = vec![
            ("details", details_mods.as_slice()),
            ("weakauras-2", wa_mods.as_slice()),
        ];
        let claimed = find_dirs_claimed_by_modules(&unmanaged, &sets);

        assert_eq!(claimed.len(), 2);
        assert_eq!(claimed["Details_DataStorage"], "details");
        assert_eq!(claimed["WeakAurasOptions"], "weakauras-2");
    }

    #[test]
    fn find_claimed_dirs_empty_modules() {
        let unmanaged = vec!["SomeDir".to_string()];
        let sets: Vec<(&str, &[String])> = vec![("addon", &[])];
        let claimed = find_dirs_claimed_by_modules(&unmanaged, &sets);
        assert!(claimed.is_empty());
    }

    // --- dir_has_toc tests ---

    #[test]
    fn dir_has_toc_with_toc_file() {
        let tmp = tempfile::tempdir().unwrap();
        write_toc(tmp.path(), "MyAddon", "MyAddon.toc", "## Title: Test");
        assert!(dir_has_toc(tmp.path(), "MyAddon"));
    }

    #[test]
    fn dir_has_toc_without_toc_file() {
        let tmp = tempfile::tempdir().unwrap();
        create_dir_with_file(tmp.path(), "ChildDir", "file.lua", "-- code");
        assert!(!dir_has_toc(tmp.path(), "ChildDir"));
    }

    #[test]
    fn dir_has_toc_nonexistent_dir() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!dir_has_toc(tmp.path(), "DoesNotExist"));
    }

    #[test]
    fn dir_has_toc_case_insensitive() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("MyAddon");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("myaddon.TOC"), "## Title: Test").unwrap();
        assert!(dir_has_toc(tmp.path(), "MyAddon"));
    }
}
