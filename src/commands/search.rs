use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::Result;
use crate::sources::AddonSource;
use crate::sources::curseforge::CurseForgeSource;

pub async fn search(query: &str, page: Option<u32>) -> Result<()> {
    let config = Config::load()?;
    let api_key = config.get_api_key()?;

    let source = CurseForgeSource::new(api_key)?;
    let result = source.search(query, page).await?;

    if result.addons.is_empty() {
        println!("No results found for '{query}'");
        return Ok(());
    }

    println!("Search results for '{}':", query.color_bold());
    println!();

    for addon in &result.addons {
        let downloads = addon
            .download_count
            .map(format_download_count)
            .unwrap_or_else(|| "N/A".to_string());

        let description = addon
            .description
            .clone()
            .unwrap_or_else(|| "No description".to_string());

        println!(
            "  {}  {}  {}",
            addon.slug.color_cyan(),
            description.color_dimmed(),
            downloads.color_green()
        );
    }

    let total_pages = result.total_pages();
    if total_pages > 1 {
        println!();
        println!(
            "  Page {} of {} ({} total results)",
            result.page, total_pages, result.total_count
        );
        if result.page < total_pages {
            println!(
                "  Use {} to see more",
                format!("--page {}", result.page + 1).color_dimmed()
            );
        }
    }

    Ok(())
}

fn format_download_count(count: u64) -> String {
    if count >= 1_000_000 {
        format!("{:.1}M downloads", count as f64 / 1_000_000.0)
    } else if count >= 1_000 {
        format!("{:.1}K downloads", count as f64 / 1_000.0)
    } else {
        format!("{count} downloads")
    }
}
