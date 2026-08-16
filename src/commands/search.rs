use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::Result;
use crate::sources::{AddonSource, SourceKind, build_source};

pub async fn search(
    query: &str,
    page: Option<u32>,
    source_filter: Option<SourceKind>,
) -> Result<()> {
    let config = Config::load()?;

    println!("Search results for '{}':", query.color_bold());

    match source_filter {
        Some(kind) => {
            // Explicit source: configuration problems are hard errors (story 8).
            let source = build_source(kind, &config)?;
            let result = source.search(query, page).await?;
            print_source_results(kind, query, &result);
        }
        None => {
            let mut kinds = vec![SourceKind::CurseForge];
            if config.get_wago_access_key().is_some() {
                kinds.push(SourceKind::Wago);
            }

            for kind in kinds {
                match build_source(kind, &config) {
                    Ok(source) => match source.search(query, page).await {
                        Ok(result) => print_source_results(kind, query, &result),
                        Err(e) => println!(
                            "  {} {} search failed: {}",
                            "Warning:".color_yellow(),
                            source_label(kind),
                            e
                        ),
                    },
                    Err(e) => println!(
                        "  {} Skipping {}: {}",
                        "Warning:".color_yellow(),
                        source_label(kind),
                        e
                    ),
                }
            }

            if config.get_wago_access_key().is_none() {
                println!();
                println!(
                    "{}",
                    "Wago: skipped (no access key configured — run 'wowctl config init' to add one)"
                        .color_dimmed()
                );
            }
        }
    }

    Ok(())
}

fn source_label(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::CurseForge => "CurseForge",
        SourceKind::Wago => "Wago",
    }
}

fn print_source_results(kind: SourceKind, query: &str, result: &crate::addon::SearchResult) {
    println!();
    println!("{}", format!("{}:", source_label(kind)).color_bold());

    if result.addons.is_empty() {
        println!("  No results found for '{query}'");
        return;
    }

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
