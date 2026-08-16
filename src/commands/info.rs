use crate::colors::ColorExt;
use crate::error::{Result, WowctlError};
use crate::registry::Registry;

pub async fn info(addon: &str) -> Result<()> {
    let registry = Registry::load()?;

    let installed_addon = registry
        .get(addon)
        .ok_or_else(|| WowctlError::AddonNotFound(format!("Addon '{addon}' is not installed")))?;

    println!("{}", installed_addon.name.color_bold());
    println!(
        "  {}: {}",
        "Version".color_bold(),
        installed_addon.version.color_green()
    );
    if let Some(display_name) = &installed_addon.display_name {
        println!(
            "  {}: {}",
            "Release".color_bold(),
            display_name.color_dimmed()
        );
    }
    println!(
        "  {}: {}",
        "Source".color_bold(),
        installed_addon.source.color_cyan()
    );
    println!(
        "  {}: {}",
        "Slug".color_bold(),
        installed_addon.slug.color_dimmed()
    );

    if !installed_addon.directories.is_empty() {
        println!("  {}:", "Directories".color_bold());
        for dir in &installed_addon.directories {
            println!("    - {dir}");
        }
    }

    if let Some(released_at) = &installed_addon.released_at {
        println!(
            "  {}: {}",
            "Released".color_bold(),
            format_release_date(released_at).color_dimmed()
        );
    }
    if let Some(game_versions) = &installed_addon.game_versions
        && !game_versions.is_empty()
    {
        println!(
            "  {}: {}",
            "Game Versions".color_bold(),
            game_versions.join(", ")
        );
    }

    if let Some(channel) = &installed_addon.channel {
        println!(
            "  {}: {}",
            "Channel".color_bold(),
            channel.to_string().color_yellow()
        );
    }

    if installed_addon.is_ignored() {
        println!(
            "  {}: {}",
            "Ignored".color_bold(),
            "yes (skipped during updates)".color_yellow()
        );
    }

    if installed_addon.is_auto_update() {
        println!(
            "  {}: {}",
            "Auto-update".color_bold(),
            "enabled".color_green()
        );
    }

    if installed_addon.is_dependency {
        println!("  {}: {}", "Type".color_bold(), "Dependency".color_yellow());
        if !installed_addon.required_by.is_empty() {
            println!("  {}:", "Required by".color_bold());
            for parent in &installed_addon.required_by {
                println!("    - {}", parent.color_cyan());
            }
        }
    } else {
        println!(
            "  {}: {}",
            "Type".color_bold(),
            "Manually installed".color_green()
        );
    }

    if let Some(url) = addon_page_url(&installed_addon.source, &installed_addon.slug) {
        println!("  {}: {}", "URL".color_bold(), url.color_blue());
    }

    Ok(())
}

/// The web page for an addon on its Source, if the Source is known.
fn addon_page_url(source: &str, slug: &str) -> Option<String> {
    match source {
        "curseforge" => Some(format!("https://www.curseforge.com/wow/addons/{slug}")),
        "wago" => Some(format!("https://addons.wago.io/addons/{slug}")),
        _ => None,
    }
}

/// Formats an ISO 8601 date string (e.g., "2025-02-15T10:30:00Z") into a
/// friendlier "YYYY-MM-DD" display. Returns the raw string if parsing fails.
///
/// Works on `chars()` rather than byte offsets: `raw` comes from an external
/// API and byte-slicing (`&raw[..10]`) panics if a multi-byte UTF-8
/// character straddles the offset.
fn format_release_date(raw: &str) -> String {
    let chars: Vec<char> = raw.chars().take(10).collect();
    if chars.len() == 10 && chars[4] == '-' && chars[7] == '-' {
        chars.into_iter().collect()
    } else {
        raw.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_release_date_iso8601() {
        assert_eq!(format_release_date("2025-02-15T10:30:00Z"), "2025-02-15");
    }

    #[test]
    fn format_release_date_date_only() {
        assert_eq!(format_release_date("2025-02-15"), "2025-02-15");
    }

    #[test]
    fn format_release_date_short_string() {
        assert_eq!(format_release_date("unknown"), "unknown");
    }

    #[test]
    fn format_release_date_empty() {
        assert_eq!(format_release_date(""), "");
    }

    #[test]
    fn format_release_date_short_non_ascii() {
        assert_eq!(format_release_date("12€"), "12€");
    }

    #[test]
    fn format_release_date_non_ascii_does_not_panic() {
        // The 10th character is a multi-byte '€'; byte-slicing at offset 10
        // would panic here.
        let raw = "1234-06-1€T00:00:00Z";
        assert_eq!(format_release_date(raw), "1234-06-1€");
    }

    #[test]
    fn addon_page_url_per_source() {
        assert_eq!(
            addon_page_url("curseforge", "weakauras-2"),
            Some("https://www.curseforge.com/wow/addons/weakauras-2".to_string())
        );
        assert_eq!(
            addon_page_url("wago", "classcodex"),
            Some("https://addons.wago.io/addons/classcodex".to_string())
        );
        assert_eq!(addon_page_url("unknown", "x"), None);
    }
}
