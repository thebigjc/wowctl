use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use dialoguer::Confirm;
use std::path::Path;

/// Removes an addon's directories from disk and from the registry.
/// Updates dependency references and returns a list of newly orphaned dependency slugs.
pub fn remove_addon_from_registry(
    registry: &mut Registry,
    slug: &str,
    addon_dir: &Path,
) -> Result<Vec<String>> {
    let installed_addon = registry
        .get(slug)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{slug}' is not installed")))?
        .clone();

    for dir_name in &installed_addon.directories {
        let dir_path = addon_dir.join(dir_name);
        if dir_path.exists() {
            std::fs::remove_dir_all(&dir_path)?;
        }
    }

    registry.remove(slug);
    registry.update_dependency_references(slug);

    Ok(registry.find_orphaned_dependencies())
}

/// Prompts to remove orphaned dependencies and removes them if confirmed.
pub fn prompt_remove_orphans(
    registry: &mut Registry,
    orphans: &[String],
    addon_dir: &Path,
) -> Result<()> {
    if orphans.is_empty() {
        return Ok(());
    }

    println!();
    println!("{}", "Orphaned dependencies:".color_yellow());
    for orphan_slug in orphans {
        if let Some(orphan) = registry.get(orphan_slug) {
            println!(
                "  {} (no other addon requires it)",
                orphan.name.color_dimmed()
            );
        }
    }

    let should_remove = Confirm::new()
        .with_prompt("Remove orphaned dependencies?")
        .default(true)
        .interact()
        .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;

    if should_remove {
        for orphan_slug in orphans {
            if let Some(orphan) = registry.get(orphan_slug).cloned() {
                println!("  Removing {}...", orphan.name.color_dimmed());
                for dir_name in &orphan.directories {
                    let dir_path = addon_dir.join(dir_name);
                    if dir_path.exists() {
                        std::fs::remove_dir_all(&dir_path)?;
                    }
                }
                registry.remove(orphan_slug);
            }
        }
    }

    Ok(())
}

pub async fn remove(addon: &str) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let mut registry = Registry::load()?;

    let installed_addon = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{addon}' is not installed")))?;
    let name = installed_addon.name.clone();

    println!("Removing {}...", name.color_cyan());

    let orphans = remove_addon_from_registry(&mut registry, addon, &addon_dir)?;
    prompt_remove_orphans(&mut registry, &orphans, &addon_dir)?;

    registry.save()?;
    println!("{}", "done.".color_green());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::InstalledAddon;
    use tempfile::tempdir;

    fn make_addon(slug: &str, dirs: Vec<&str>) -> InstalledAddon {
        InstalledAddon {
            name: slug.to_string(),
            slug: slug.to_string(),
            version: "1.0.0".to_string(),
            source: "curseforge".to_string(),
            addon_id: "1".to_string(),
            directories: dirs.into_iter().map(|s| s.to_string()).collect(),
            is_dependency: false,
            required_by: vec![],
            installed_file_id: None,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: None,
            auto_update: None,
        }
    }

    #[test]
    fn remove_addon_deletes_directories() {
        let tmp = tempdir().unwrap();
        let dir1 = tmp.path().join("MyAddon");
        let dir2 = tmp.path().join("MyAddon_Options");
        std::fs::create_dir_all(&dir1).unwrap();
        std::fs::create_dir_all(&dir2).unwrap();

        let mut registry = Registry::default();
        registry.add(make_addon("my-addon", vec!["MyAddon", "MyAddon_Options"]));

        remove_addon_from_registry(&mut registry, "my-addon", tmp.path()).unwrap();

        assert!(!dir1.exists());
        assert!(!dir2.exists());
    }

    #[test]
    fn remove_addon_updates_registry() {
        let tmp = tempdir().unwrap();
        let mut registry = Registry::default();
        registry.add(make_addon("my-addon", vec![]));

        remove_addon_from_registry(&mut registry, "my-addon", tmp.path()).unwrap();

        assert!(registry.get("my-addon").is_none());
    }

    #[test]
    fn remove_addon_returns_orphaned_deps() {
        let tmp = tempdir().unwrap();
        let mut registry = Registry::default();

        let mut parent = make_addon("parent-addon", vec![]);
        parent.is_dependency = false;

        let mut dep = make_addon("dep-lib", vec![]);
        dep.is_dependency = true;
        dep.required_by = vec!["parent-addon".to_string()];

        registry.add(parent);
        registry.add(dep);

        let orphans =
            remove_addon_from_registry(&mut registry, "parent-addon", tmp.path()).unwrap();

        assert_eq!(orphans, vec!["dep-lib".to_string()]);
    }

    #[test]
    fn remove_addon_skips_nonexistent_dirs() {
        let tmp = tempdir().unwrap();
        let mut registry = Registry::default();
        registry.add(make_addon(
            "ghost-addon",
            vec!["DoesNotExist", "AlsoMissing"],
        ));

        let result = remove_addon_from_registry(&mut registry, "ghost-addon", tmp.path());
        assert!(result.is_ok());
        assert!(registry.get("ghost-addon").is_none());
    }
}
