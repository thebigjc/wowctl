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

/// Formats the release-date marker shown next to a managed addon (e.g.
/// "  2025-02-15"), or an empty string if no release date is recorded.
/// Truncates on a char boundary — release dates come from an external API
/// and are not guaranteed to be pure ASCII.
fn format_date_marker(released_at: Option<&str>) -> String {
    released_at
        .map(|d| crate::utils::char_safe_prefix(d, 10))
        .map(|d| format!("  {d}"))
        .unwrap_or_default()
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
                    let date_str = format_date_marker(addon.released_at.as_deref());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_date_marker_none_is_empty() {
        assert_eq!(format_date_marker(None), "");
    }

    #[test]
    fn format_date_marker_full_iso_date() {
        assert_eq!(
            format_date_marker(Some("2025-02-15T10:30:00Z")),
            "  2025-02-15"
        );
    }

    #[test]
    fn format_date_marker_short_input() {
        assert_eq!(format_date_marker(Some("2025")), "  2025");
    }

    #[test]
    fn format_date_marker_non_ascii_does_not_panic() {
        // The 10th character is a multi-byte '€'; byte-slicing at offset 10
        // would panic here.
        let raw = "1234-06-1€T00:00:00Z";
        assert_eq!(format_date_marker(Some(raw)), "  1234-06-1€");
    }
}
