use crate::colors::ColorExt;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;

pub async fn enable(addon: &str) -> Result<()> {
    let mut registry = Registry::load()?;

    let installed = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{}' is not installed", addon)))?;

    if installed.is_auto_update() {
        println!("{} already has auto-update enabled.", addon.color_cyan());
        return Ok(());
    }

    let mut updated = installed.clone();
    updated.auto_update = Some(true);
    registry.add(updated);
    registry.save()?;

    println!("{} will be automatically updated.", addon.color_cyan());

    Ok(())
}

pub async fn disable(addon: &str) -> Result<()> {
    let mut registry = Registry::load()?;

    let installed = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{}' is not installed", addon)))?;

    if !installed.is_auto_update() {
        println!("{} does not have auto-update enabled.", addon.color_cyan());
        return Ok(());
    }

    let mut updated = installed.clone();
    updated.auto_update = None;
    registry.add(updated);
    registry.save()?;

    println!(
        "{} will no longer be automatically updated.",
        addon.color_cyan()
    );

    Ok(())
}
