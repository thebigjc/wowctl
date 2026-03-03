use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;
use dialoguer::Confirm;

pub async fn remove(addon: &str) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let mut registry = Registry::load()?;

    let installed_addon = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{}' is not installed", addon)))?
        .clone();

    println!("Removing {}...", installed_addon.name.color_cyan());

    for dir_name in &installed_addon.directories {
        let dir_path = addon_dir.join(dir_name);
        if dir_path.exists() {
            std::fs::remove_dir_all(&dir_path)?;
        }
    }

    registry.remove(addon);
    registry.update_dependency_references(addon);

    let orphans = registry.find_orphaned_dependencies();

    if !orphans.is_empty() {
        println!();
        println!("{}", "Orphaned dependencies:".color_yellow());
        for orphan_slug in &orphans {
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
            .map_err(|e| WowctlError::Config(format!("Failed to read input: {}", e)))?;

        if should_remove {
            for orphan_slug in orphans {
                if let Some(orphan) = registry.get(&orphan_slug).cloned() {
                    println!("  Removing {}...", orphan.name.color_dimmed());
                    for dir_name in &orphan.directories {
                        let dir_path = addon_dir.join(dir_name);
                        if dir_path.exists() {
                            std::fs::remove_dir_all(&dir_path)?;
                        }
                    }
                    registry.remove(&orphan_slug);
                }
            }
        }
    }

    registry.save()?;
    println!("{}", "done.".color_green());

    Ok(())
}
