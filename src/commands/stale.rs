use crate::addon::InstalledAddon;
use crate::colors::ColorExt;
use crate::commands::remove::{prompt_remove_orphans, remove_addon_from_registry};
use crate::config::Config;
use crate::error::Result;
use crate::registry::Registry;
use dialoguer::Confirm;

/// Parses a `YYYY-MM-DD` date from the start of a string (ISO 8601 or date-only).
/// Returns `(year, month, day)` or `None` if the format is invalid.
fn parse_date_prefix(s: &str) -> Option<(i32, u32, u32)> {
    if s.len() < 10 {
        return None;
    }
    let bytes = s.as_bytes();
    if bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let year: i32 = s[0..4].parse().ok()?;
    let month: u32 = s[5..7].parse().ok()?;
    let day: u32 = s[8..10].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    Some((year, month, day))
}

/// Returns the current date as `(year, month, day)`.
fn today() -> (i32, u32, u32) {
    // Use UNIX timestamp to derive the date without pulling in chrono.
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;

    // Days since epoch
    let days = (secs / 86400) as i32;

    // Civil date from day count (algorithm from Howard Hinnant)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i32 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    (y, m, d)
}

/// Returns the number of months between two dates (approximate, calendar months).
fn months_between(earlier: (i32, u32, u32), later: (i32, u32, u32)) -> u32 {
    let year_diff = later.0 - earlier.0;
    let month_diff = later.1 as i32 - earlier.1 as i32;
    let day_adjust = if later.2 < earlier.2 { -1 } else { 0 };
    (year_diff * 12 + month_diff + day_adjust).max(0) as u32
}

/// Human-readable age string from a month count.
fn format_age(months: u32) -> String {
    if months < 1 {
        "< 1 month".to_string()
    } else if months == 1 {
        "1 month".to_string()
    } else if months < 24 {
        format!("{months} months")
    } else {
        let years = months / 12;
        let rem = months % 12;
        if rem == 0 {
            format!("{years} years")
        } else {
            format!("{years} years, {rem} months")
        }
    }
}

/// Partition addons into (stale, unknown_date) buckets.
/// Ignored addons are excluded entirely.
/// Dependencies that are still required by a non-stale addon are excluded from the stale bucket.
fn find_stale_addons(
    registry: &Registry,
    threshold_months: u32,
) -> (Vec<(InstalledAddon, u32)>, Vec<InstalledAddon>) {
    let now = today();
    let all: Vec<&InstalledAddon> = registry.list_all();

    let mut stale: Vec<(InstalledAddon, u32)> = Vec::new();
    let mut unknown: Vec<InstalledAddon> = Vec::new();

    for addon in &all {
        if addon.is_ignored() {
            continue;
        }

        match &addon.released_at {
            Some(date_str) => {
                if let Some(date) = parse_date_prefix(date_str) {
                    let age = months_between(date, now);
                    if age >= threshold_months {
                        stale.push(((*addon).clone(), age));
                    }
                } else {
                    unknown.push((*addon).clone());
                }
            }
            None => {
                unknown.push((*addon).clone());
            }
        }
    }

    // Exclude dependencies that are still required by a non-stale addon.
    let stale_slugs: Vec<String> = stale.iter().map(|(a, _)| a.slug.clone()).collect();
    stale.retain(|(addon, _)| {
        if !addon.is_dependency || addon.required_by.is_empty() {
            return true;
        }
        // Keep only if ALL parents are also stale (or removed).
        addon
            .required_by
            .iter()
            .all(|parent| stale_slugs.contains(parent))
    });

    // Sort oldest first, break ties by slug.
    stale.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.slug.cmp(&b.0.slug)));

    (stale, unknown)
}

pub async fn stale(threshold_months: u32) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let mut registry = Registry::load()?;

    let (stale, unknown) = find_stale_addons(&registry, threshold_months);

    if stale.is_empty() && unknown.is_empty() {
        println!(
            "{}",
            format!(
                "No stale addons found (threshold: {threshold_months} months)."
            )
            .color_green()
        );
        return Ok(());
    }

    if !stale.is_empty() {
        println!(
            "{}",
            format!(
                "Addons with no update in {threshold_months} months or more:"
            )
            .color_bold()
        );
        println!();

        // Find longest slug for alignment.
        let max_slug = stale.iter().map(|(a, _)| a.slug.len()).max().unwrap_or(0);
        let max_ver = stale
            .iter()
            .map(|(a, _)| a.version.len())
            .max()
            .unwrap_or(0);

        for (addon, age) in &stale {
            let date = addon
                .released_at
                .as_deref()
                .and_then(|d| if d.len() >= 10 { Some(&d[..10]) } else { None })
                .unwrap_or("unknown");
            println!(
                "  {:<slug_w$}  {:<ver_w$}  {}  ({})",
                addon.slug.color_cyan(),
                addon.version.color_green(),
                date.color_dimmed(),
                format_age(*age).color_yellow(),
                slug_w = max_slug,
                ver_w = max_ver,
            );
        }
        println!();

        for (addon, age) in &stale {
            let prompt = format!(
                "Remove {} ({}, last updated {} ago)?",
                addon.name,
                addon.version,
                format_age(*age),
            );
            let should_remove = Confirm::new()
                .with_prompt(prompt)
                .default(false)
                .interact()
                .unwrap_or(false);

            if should_remove {
                println!("  Removing {}...", addon.name.color_cyan());
                let orphans =
                    remove_addon_from_registry(&mut registry, &addon.slug, &addon_dir)?;
                prompt_remove_orphans(&mut registry, &orphans, &addon_dir)?;
            }
        }

        registry.save()?;
    }

    if !unknown.is_empty() {
        println!();
        println!(
            "{}",
            "Addons with unknown release date (run `wowctl update` to backfill):".color_yellow()
        );
        for addon in &unknown {
            println!("  {}  {}", addon.slug.color_cyan(), addon.version.color_green());
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::addon::InstalledAddon;

    fn make_addon(slug: &str, released_at: Option<&str>) -> InstalledAddon {
        InstalledAddon {
            name: slug.to_string(),
            slug: slug.to_string(),
            version: "1.0.0".to_string(),
            source: "curseforge".to_string(),
            addon_id: "1".to_string(),
            directories: vec![],
            is_dependency: false,
            required_by: vec![],
            installed_file_id: None,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: released_at.map(|s| s.to_string()),
            auto_update: None,
        }
    }

    fn date_months_ago(months: u32) -> String {
        let (y, m, d) = today();
        let total_months = y * 12 + m as i32 - months as i32;
        let new_y = (total_months - 1) / 12;
        let new_m = ((total_months - 1) % 12 + 1) as u32;
        let new_d = d.min(28); // Safe day to avoid month overflow
        format!("{new_y:04}-{new_m:02}-{new_d:02}T00:00:00Z")
    }

    // --- Date parsing tests ---

    #[test]
    fn parse_released_date_iso8601() {
        let result = parse_date_prefix("2025-02-15T10:30:00Z");
        assert_eq!(result, Some((2025, 2, 15)));
    }

    #[test]
    fn parse_released_date_date_only() {
        let result = parse_date_prefix("2025-02-15");
        assert_eq!(result, Some((2025, 2, 15)));
    }

    #[test]
    fn parse_released_date_invalid_returns_none() {
        assert_eq!(parse_date_prefix("not-a-date"), None);
    }

    #[test]
    fn parse_released_date_empty_returns_none() {
        assert_eq!(parse_date_prefix(""), None);
    }

    // --- Age calculation tests ---

    #[test]
    fn age_months_calculation() {
        let age = months_between((2025, 1, 15), (2025, 5, 20));
        assert_eq!(age, 4);
    }

    #[test]
    fn age_months_boundary() {
        // Same day of month — should count as exactly N months
        let age = months_between((2025, 1, 15), (2025, 4, 15));
        assert_eq!(age, 3);
    }

    #[test]
    fn age_months_day_before_reduces_count() {
        // Day is earlier in the month → one fewer month
        let age = months_between((2025, 1, 20), (2025, 4, 15));
        assert_eq!(age, 2);
    }

    // --- Filtering tests ---

    #[test]
    fn filters_out_ignored_addons() {
        let mut registry = Registry::default();
        let mut addon = make_addon("old-addon", Some("2020-01-01T00:00:00Z"));
        addon.ignored = Some(true);
        registry.add(addon);

        let (stale, unknown) = find_stale_addons(&registry, 3);
        assert!(stale.is_empty());
        assert!(unknown.is_empty());
    }

    #[test]
    fn includes_non_ignored_stale_addon() {
        let mut registry = Registry::default();
        registry.add(make_addon("old-addon", Some("2020-01-01T00:00:00Z")));

        let (stale, _) = find_stale_addons(&registry, 3);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].0.slug, "old-addon");
    }

    #[test]
    fn excludes_addon_within_threshold() {
        let mut registry = Registry::default();
        let recent = date_months_ago(1);
        registry.add(make_addon("fresh-addon", Some(&recent)));

        let (stale, _) = find_stale_addons(&registry, 3);
        assert!(stale.is_empty());
    }

    #[test]
    fn includes_addon_beyond_threshold() {
        let mut registry = Registry::default();
        let old = date_months_ago(6);
        registry.add(make_addon("stale-addon", Some(&old)));

        let (stale, _) = find_stale_addons(&registry, 3);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].0.slug, "stale-addon");
    }

    #[test]
    fn missing_released_at_separated() {
        let mut registry = Registry::default();
        registry.add(make_addon("no-date-addon", None));

        let (stale, unknown) = find_stale_addons(&registry, 3);
        assert!(stale.is_empty());
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].slug, "no-date-addon");
    }

    #[test]
    fn dependency_of_fresh_addon_excluded() {
        let mut registry = Registry::default();

        // Fresh parent
        let recent = date_months_ago(1);
        registry.add(make_addon("parent-addon", Some(&recent)));

        // Old dependency required by the fresh parent
        let mut dep = make_addon("old-lib", Some("2020-01-01T00:00:00Z"));
        dep.is_dependency = true;
        dep.required_by = vec!["parent-addon".to_string()];
        registry.add(dep);

        let (stale, _) = find_stale_addons(&registry, 3);
        // The old dependency should NOT appear because its parent is fresh
        assert!(
            stale.iter().all(|(a, _)| a.slug != "old-lib"),
            "dependency of fresh addon should be excluded"
        );
    }

    #[test]
    fn orphaned_stale_dependency_included() {
        let mut registry = Registry::default();
        let mut dep = make_addon("orphan-lib", Some("2020-01-01T00:00:00Z"));
        dep.is_dependency = true;
        dep.required_by = vec![];
        registry.add(dep);

        let (stale, _) = find_stale_addons(&registry, 3);
        assert_eq!(stale.len(), 1);
        assert_eq!(stale[0].0.slug, "orphan-lib");
    }

    // --- Sorting tests ---

    #[test]
    fn stale_addons_sorted_oldest_first() {
        let mut registry = Registry::default();
        registry.add(make_addon("four-mo", Some(&date_months_ago(4))));
        registry.add(make_addon("twelve-mo", Some(&date_months_ago(12))));
        registry.add(make_addon("eight-mo", Some(&date_months_ago(8))));

        let (stale, _) = find_stale_addons(&registry, 3);
        let slugs: Vec<&str> = stale.iter().map(|(a, _)| a.slug.as_str()).collect();
        assert_eq!(slugs, vec!["twelve-mo", "eight-mo", "four-mo"]);
    }

    #[test]
    fn stale_addons_same_date_stable_sort() {
        let mut registry = Registry::default();
        let date = date_months_ago(6);
        registry.add(make_addon("bbb-addon", Some(&date)));
        registry.add(make_addon("aaa-addon", Some(&date)));

        let (stale, _) = find_stale_addons(&registry, 3);
        let slugs: Vec<&str> = stale.iter().map(|(a, _)| a.slug.as_str()).collect();
        // Same age → alphabetical by slug
        assert_eq!(slugs, vec!["aaa-addon", "bbb-addon"]);
    }

    // --- format_age tests ---

    #[test]
    fn format_age_singular() {
        assert_eq!(format_age(1), "1 month");
    }

    #[test]
    fn format_age_plural_months() {
        assert_eq!(format_age(5), "5 months");
    }

    #[test]
    fn format_age_years() {
        assert_eq!(format_age(24), "2 years");
    }

    #[test]
    fn format_age_years_and_months() {
        assert_eq!(format_age(26), "2 years, 2 months");
    }

    #[test]
    fn format_age_less_than_one() {
        assert_eq!(format_age(0), "< 1 month");
    }
}
