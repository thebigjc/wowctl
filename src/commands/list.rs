use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::Result;
use crate::registry::Registry;
use crate::utils::{dir_has_toc, get_unmanaged_dirs};

pub enum ListFilter {
    All,
    Managed,
    Unmanaged,
}

pub async fn list(filter: ListFilter) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let registry = Registry::load()?;

    if !addon_dir.exists() {
        println!(
            "{}",
            format!("Addon directory does not exist: {}", addon_dir.display()).color_red()
        );
        return Ok(());
    }

    let managed_addons = registry.list_all();
    let unmanaged_dirs = get_unmanaged_dirs(&addon_dir, &registry)?;

    match filter {
        ListFilter::All | ListFilter::Managed => {
            if !managed_addons.is_empty() {
                println!("{}", "Managed addons:".color_bold());
                for addon in managed_addons {
                    let mut markers = String::new();
                    if addon.is_dependency {
                        markers.push_str(" (dependency)");
                    }
                    if addon.is_ignored() {
                        markers.push_str(" (ignored)");
                    }
                    if addon.is_auto_update() {
                        markers.push_str(" (auto-update)");
                    }
                    let date_str = addon
                        .released_at
                        .as_deref()
                        .map(|d| if d.len() >= 10 { &d[..10] } else { d })
                        .map(|d| format!("  {}", d))
                        .unwrap_or_default();
                    println!(
                        "  {}  {}  {}{}{}",
                        addon.slug.color_cyan(),
                        addon.version.color_green(),
                        addon.source.color_dimmed(),
                        date_str.color_dimmed(),
                        markers.color_dimmed()
                    );
                }
                println!();
            } else if matches!(filter, ListFilter::Managed) {
                println!("No managed addons found.");
                return Ok(());
            }
        }
        _ => {}
    }

    match filter {
        ListFilter::All | ListFilter::Unmanaged => {
            if !unmanaged_dirs.is_empty() {
                println!("{}", "Unmanaged addons:".color_bold());
                for dir in &unmanaged_dirs {
                    if dir_has_toc(&addon_dir, dir) {
                        println!("  {}", dir.color_yellow());
                    } else {
                        println!(
                            "  {} {}",
                            dir.color_yellow(),
                            "(no .toc — possibly a child of another addon)".color_dimmed()
                        );
                    }
                }
            } else if matches!(filter, ListFilter::Unmanaged) {
                println!("No unmanaged addons found.");
            }
        }
        _ => {}
    }

    Ok(())
}
