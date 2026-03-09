use clap::{Parser, Subcommand};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use wowctl::addon::ReleaseChannel;
use wowctl::error::Result;

#[derive(Parser)]
#[command(name = "wowctl")]
#[command(about = "World of Warcraft Addon Manager", long_about = None)]
#[command(version = env!("WOWCTL_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    #[arg(long, global = true, help = "Disable colored output")]
    no_color: bool,

    #[arg(long, global = true, help = "Enable verbose/debug logging")]
    verbose: bool,

    #[arg(
        long,
        global = true,
        help = "Override the addon directory for this invocation"
    )]
    addon_dir: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    #[command(about = "Configure wowctl")]
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    #[command(about = "Install an addon")]
    Install {
        #[arg(help = "Addon name, slug, or CurseForge URL")]
        addon: String,

        #[arg(long, value_enum, help = "Release channel: stable, beta, or alpha")]
        channel: Option<ReleaseChannel>,
    },

    #[command(about = "Update addon(s)")]
    Update {
        #[arg(help = "Specific addon to update (updates all if not specified)")]
        addon: Option<String>,

        #[arg(long, help = "Skip confirmation and install all updates")]
        auto: bool,

        #[arg(
            long,
            help = "Only update addons with auto-update enabled (implies --auto)"
        )]
        auto_only: bool,

        #[arg(long, value_enum, help = "Release channel: stable, beta, or alpha")]
        channel: Option<ReleaseChannel>,
    },

    #[command(about = "Remove an addon")]
    Remove {
        #[arg(help = "Addon name or slug to remove")]
        addon: String,
    },

    #[command(about = "List installed addons")]
    List {
        #[arg(long, help = "Show only managed addons")]
        managed: bool,

        #[arg(long, help = "Show only unmanaged addons")]
        unmanaged: bool,
    },

    #[command(about = "Search for addons")]
    Search {
        #[arg(help = "Search query")]
        query: String,

        #[arg(long, help = "Page number (default: 1)")]
        page: Option<u32>,
    },

    #[command(about = "Show detailed addon information")]
    Info {
        #[arg(help = "Addon name or slug")]
        addon: String,
    },

    #[command(about = "Adopt an unmanaged addon")]
    Adopt {
        #[arg(help = "Addon folder name (omit with --all to adopt everything)")]
        addon_folder: Option<String>,

        #[arg(long, help = "Adopt all unmanaged addons")]
        all: bool,

        #[arg(long, help = "CurseForge slug (for single-folder adopt)")]
        slug: Option<String>,
    },

    #[command(about = "Ignore an addon (skip during update checks)")]
    Ignore {
        #[arg(help = "Addon slug to ignore")]
        addon: String,
    },

    #[command(about = "Unignore an addon (include in update checks again)")]
    Unignore {
        #[arg(help = "Addon slug to unignore")]
        addon: String,
    },

    #[command(name = "auto-update", about = "Enable auto-update for an addon")]
    AutoUpdate {
        #[arg(help = "Addon slug to enable auto-update for")]
        addon: String,
    },

    #[command(name = "no-auto-update", about = "Disable auto-update for an addon")]
    NoAutoUpdate {
        #[arg(help = "Addon slug to disable auto-update for")]
        addon: String,
    },
}

#[derive(Subcommand)]
enum ConfigAction {
    #[command(about = "Interactive first-time setup")]
    Init,

    #[command(about = "Display current configuration")]
    Show,

    #[command(about = "Set a configuration value")]
    Set {
        #[arg(help = "Configuration key")]
        key: String,

        #[arg(help = "Configuration value")]
        value: String,
    },
}

fn init_logging(verbose: bool) {
    let filter = if verbose {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("wowctl=debug,info"))
    } else {
        EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("wowctl=info,warn,error"))
    };

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    init_logging(cli.verbose);

    let use_color = !cli.no_color && std::env::var("NO_COLOR").is_err();
    wowctl::colors::set_colors_enabled(use_color);
    tracing::debug!(
        "Color output: {}",
        if use_color { "enabled" } else { "disabled" }
    );

    if let Some(ref addon_dir) = cli.addon_dir {
        unsafe {
            std::env::set_var("WOWCTL_ADDON_DIR_OVERRIDE", addon_dir);
        }
        tracing::debug!("Addon directory override: {}", addon_dir);
    }

    tracing::debug!("wowctl starting with verbose logging enabled");
    tracing::info!("wowctl version {}", env!("WOWCTL_VERSION"));

    match cli.command {
        Commands::Config { action } => match action {
            ConfigAction::Init => wowctl::commands::config::init().await,
            ConfigAction::Show => wowctl::commands::config::show().await,
            ConfigAction::Set { key, value } => wowctl::commands::config::set(&key, &value).await,
        },
        Commands::Install { addon, channel } => {
            wowctl::commands::install::install(&addon, channel).await
        }
        Commands::Update {
            addon,
            auto,
            auto_only,
            channel,
        } => {
            wowctl::commands::update::update(
                addon.as_deref(),
                auto || auto_only,
                auto_only,
                channel,
            )
            .await
        }
        Commands::Remove { addon } => wowctl::commands::remove::remove(&addon).await,
        Commands::List { managed, unmanaged } => {
            let filter = if managed {
                wowctl::commands::list::ListFilter::Managed
            } else if unmanaged {
                wowctl::commands::list::ListFilter::Unmanaged
            } else {
                wowctl::commands::list::ListFilter::All
            };
            wowctl::commands::list::list(filter).await
        }
        Commands::Search { query, page } => wowctl::commands::search::search(&query, page).await,
        Commands::Info { addon } => wowctl::commands::info::info(&addon).await,
        Commands::Adopt {
            addon_folder,
            all,
            slug,
        } => wowctl::commands::adopt::adopt(addon_folder.as_deref(), all, slug.as_deref()).await,
        Commands::Ignore { addon } => wowctl::commands::ignore::ignore(&addon).await,
        Commands::Unignore { addon } => wowctl::commands::ignore::unignore(&addon).await,
        Commands::AutoUpdate { addon } => wowctl::commands::auto_update::enable(&addon).await,
        Commands::NoAutoUpdate { addon } => wowctl::commands::auto_update::disable(&addon).await,
    }
}
