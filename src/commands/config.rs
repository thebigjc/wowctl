use crate::colors::ColorExt;
use crate::config::Config;
use crate::error::{Result, WowctlError};
use crate::sources::AddonSource;
use crate::sources::curseforge::CurseForgeSource;
use dialoguer::{Confirm, Input};
use std::path::PathBuf;

pub async fn init() -> Result<()> {
    println!("{}", "wowctl configuration setup".color_bold());
    println!();

    let mut config = Config::load().unwrap_or_default();

    println!("{}", "Step 1: CurseForge API Key".color_bold());
    println!("You need a CurseForge API key to use wowctl.");
    println!("Get one at: https://console.curseforge.com/");
    println!();

    let api_key: String = Input::new()
        .with_prompt("Enter your CurseForge API key")
        .interact_text()
        .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;

    println!();
    println!("{}", "Verifying API key...".color_dimmed());

    let test_source = CurseForgeSource::new(api_key.clone())?;
    test_source
        .search("deadly-boss-mods", None)
        .await
        .map_err(|_| {
            WowctlError::Config(
                "API key verification failed. Please check your key and try again.".to_string(),
            )
        })?;

    println!("{}", "API key verified successfully!".color_green());
    config.curseforge_api_key = Some(api_key);
    println!();

    println!("{}", "Step 2: Addon Directory".color_bold());

    match Config::detect_addon_dir() {
        Ok(detected_path) => {
            println!(
                "Detected addon directory: {}",
                detected_path.display().to_string().color_cyan()
            );
            let use_detected = Confirm::new()
                .with_prompt("Use this directory?")
                .default(true)
                .interact()
                .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;

            if use_detected {
                config.addon_dir = Some(detected_path);
            } else {
                let custom_path: String = Input::new()
                    .with_prompt("Enter addon directory path")
                    .interact_text()
                    .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;
                config.addon_dir = Some(PathBuf::from(custom_path));
            }
        }
        Err(_) => {
            println!(
                "{}",
                "Could not auto-detect addon directory.".color_yellow()
            );
            let custom_path: String = Input::new()
                .with_prompt("Enter addon directory path")
                .interact_text()
                .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;
            config.addon_dir = Some(PathBuf::from(custom_path));
        }
    }

    if let Some(ref addon_dir) = config.addon_dir
        && !addon_dir.exists()
    {
        println!(
            "{}",
            format!("Warning: Directory does not exist: {}", addon_dir.display()).color_yellow()
        );
    }

    println!();
    let data_dir = Config::data_dir()?;
    if !data_dir.exists() {
        std::fs::create_dir_all(&data_dir)?;
        println!("Created data directory: {}", data_dir.display());
    }

    config.save()?;

    println!();
    println!(
        "{}",
        "Configuration saved successfully!"
            .color_green()
            .color_bold()
    );
    println!("Config file: {}", Config::config_path()?.display());

    Ok(())
}

pub async fn show() -> Result<()> {
    let config = Config::load()?;

    println!("{}", "Current configuration:".color_bold());
    println!();

    println!(
        "  {}: {}",
        "Config file".color_bold(),
        Config::config_path()?.display()
    );

    println!(
        "  {}: {}",
        "Data directory".color_bold(),
        Config::data_dir()?.display()
    );

    println!();

    match &config.curseforge_api_key {
        Some(key) => {
            let masked = mask_api_key(key);
            println!(
                "  {}: {}",
                "CurseForge API key".color_bold(),
                masked.color_green()
            );
        }
        None => {
            println!(
                "  {}: {}",
                "CurseForge API key".color_bold(),
                "Not set".color_red()
            );
        }
    }

    match &config.addon_dir {
        Some(dir) => {
            println!(
                "  {}: {}",
                "Addon directory".color_bold(),
                dir.display().to_string().color_cyan()
            );
            if dir.exists() {
                println!("    {}", "(exists)".color_green());
            } else {
                println!("    {}", "(does not exist)".color_red());
            }
        }
        None => {
            println!(
                "  {}: {}",
                "Addon directory".color_bold(),
                "Not set (will auto-detect)".color_yellow()
            );
        }
    }

    print!("  {}: ", "Color output".color_bold());
    if config.color {
        println!("{}", "enabled".color_green());
    } else {
        println!("{}", "disabled".color_red());
    }

    match &config.default_release_channel {
        Some(channel) => {
            println!(
                "  {}: {}",
                "Release channel".color_bold(),
                channel.to_string().color_yellow()
            );
        }
        None => {
            println!(
                "  {}: {}",
                "Release channel".color_bold(),
                "stable (default)".color_green()
            );
        }
    }

    Ok(())
}

pub async fn set(key: &str, value: &str) -> Result<()> {
    let mut config = Config::load()?;

    match key {
        "addon_dir" => {
            let path = PathBuf::from(value);
            config.addon_dir = Some(path.clone());
            println!("Set addon directory to: {}", path.display());
        }
        "curseforge_api_key" => {
            config.curseforge_api_key = Some(value.to_string());
            println!("Set CurseForge API key");
        }
        "color" => {
            let color_value = value.parse::<bool>().map_err(|_| {
                WowctlError::Config(format!(
                    "Invalid boolean value: '{value}'. Use 'true' or 'false'"
                ))
            })?;
            config.color = color_value;
            println!("Set color output to: {color_value}");
        }
        "default_release_channel" | "channel" => {
            let channel: crate::addon::ReleaseChannel =
                value.parse().map_err(|e: String| WowctlError::Config(e))?;
            config.default_release_channel = Some(channel);
            println!("Set default release channel to: {channel}");
        }
        _ => {
            return Err(WowctlError::Config(format!(
                "Unknown configuration key: '{key}'. Valid keys: addon_dir, curseforge_api_key, color, default_release_channel"
            )));
        }
    }

    config.save()?;
    println!("{}", "Configuration saved.".color_green());

    Ok(())
}

fn mask_api_key(key: &str) -> String {
    if key.len() <= 8 {
        return "*".repeat(key.len());
    }

    let prefix = &key[..4];
    let suffix = &key[key.len() - 4..];
    format!("{prefix}...{suffix}")
}
