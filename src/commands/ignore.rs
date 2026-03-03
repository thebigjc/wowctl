use crate::colors::ColorExt;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;

pub async fn ignore(addon: &str) -> Result<()> {
    let mut registry = Registry::load()?;

    let installed = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{}' is not installed", addon)))?;

    if installed.is_ignored() {
        println!("{} is already ignored.", addon.color_cyan());
        return Ok(());
    }

    let mut updated = installed.clone();
    updated.ignored = Some(true);
    registry.add(updated);
    registry.save()?;

    println!(
        "{} will be skipped during update checks.",
        addon.color_cyan()
    );

    Ok(())
}

pub async fn unignore(addon: &str) -> Result<()> {
    let mut registry = Registry::load()?;

    let installed = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{}' is not installed", addon)))?;

    if !installed.is_ignored() {
        println!("{} is not ignored.", addon.color_cyan());
        return Ok(());
    }

    let mut updated = installed.clone();
    updated.ignored = None;
    registry.add(updated);
    registry.save()?;

    println!("{} will be included in update checks.", addon.color_cyan());

    Ok(())
}
