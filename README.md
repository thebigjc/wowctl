# wowctl — World of Warcraft Addon Manager CLI

A fast, reliable command-line tool for managing World of Warcraft Retail addons on macOS and Windows.

## Features

- **Fast & Reliable**: Written in Rust for performance and cross-platform portability
- **Automatic Dependency Resolution**: Install addons with all their required dependencies automatically
- **CurseForge Integration**: Search, install, and update addons from CurseForge
- **Smart Updates**: Check for updates across all your addons with a single command
- **Orphan Cleanup**: Automatically detect and remove orphaned dependencies
- **Colored Output**: Beautiful terminal output with color support
- **Extensible Architecture**: Trait-based design supports additional addon sources in the future

## Installation

### From Source

```bash
git clone https://github.com/yourusername/wowctl.git
cd wowctl
cargo build --release
```

The binary will be at `target/release/wowctl`. Add it to your PATH for easy access.

## Quick Start

### 1. Initial Setup

Run the interactive configuration wizard:

```bash
wowctl config init
```

This will:
- Prompt for your CurseForge API key (get one at https://console.curseforge.com/)
- Auto-detect your WoW addon directory
- Verify everything works

### 2. Search for Addons

```bash
wowctl search "deadly boss mods"
```

### 3. Install an Addon

```bash
wowctl install deadly-boss-mods
```

Or use a CurseForge URL:

```bash
wowctl install https://www.curseforge.com/wow/addons/deadly-boss-mods
```

Dependencies are installed automatically!

### 4. List Installed Addons

```bash
wowctl list
```

### 5. Update Addons

Check for updates:

```bash
wowctl update
```

Install all updates without prompting:

```bash
wowctl update --auto
```

Update a specific addon:

```bash
wowctl update deadly-boss-mods
```

### 6. Remove an Addon

```bash
wowctl remove deadly-boss-mods
```

Orphaned dependencies will be detected and you'll be prompted to remove them.

## Commands

### `wowctl config`

Manage configuration.

- `wowctl config init` — Interactive first-time setup
- `wowctl config show` — Display current configuration
- `wowctl config set <key> <value>` — Set a configuration value

**Configuration Keys:**
- `addon_dir` — Path to WoW addon directory
- `curseforge_api_key` — Your CurseForge API key
- `color` — Enable/disable colored output (`true` or `false`)

### `wowctl search <query>`

Search CurseForge for addons.

```bash
wowctl search "boss mods"
```

### `wowctl install <addon>`

Install an addon with its dependencies.

```bash
wowctl install deadly-boss-mods
wowctl install https://www.curseforge.com/wow/addons/details
```

### `wowctl update [addon]`

Check for and install updates.

```bash
wowctl update                    # Check all addons
wowctl update deadly-boss-mods   # Check specific addon
wowctl update --auto             # Install all updates without prompting
```

### `wowctl remove <addon>`

Remove an installed addon.

```bash
wowctl remove deadly-boss-mods
```

### `wowctl list`

List all addons in your addon directory.

```bash
wowctl list              # Show all addons
wowctl list --managed    # Show only wowctl-managed addons
wowctl list --unmanaged  # Show only unmanaged addons
```

### `wowctl info <addon>`

Show detailed information about an installed addon.

```bash
wowctl info deadly-boss-mods
```

### `wowctl adopt <folder>`

Adopt an unmanaged addon (Phase 2 feature - not yet implemented).

## Global Flags

- `--no-color` — Disable colored output
- `--verbose` — Enable verbose/debug logging
- `--addon-dir <path>` — Override the addon directory for this command
- `--help` — Show help
- `--version` — Show version

## Configuration

### Config File Location

- **macOS**: `~/Library/Application Support/wowctl/config.toml`
- **Windows**: `%APPDATA%\wowctl\config.toml`

### Data Directory

- **macOS**: `~/Library/Application Support/wowctl/`
- **Windows**: `%LOCALAPPDATA%\wowctl\`

The data directory contains the addon registry (`registry.toml`) which tracks all managed addons.

### Environment Variables

- `WOWCTL_CURSEFORGE_API_KEY` — CurseForge API key (takes precedence over config file)
- `NO_COLOR` — Disable colored output (respects standard)
- `RUST_LOG` — Control logging verbosity (e.g., `RUST_LOG=debug wowctl search foo`)

## Default Addon Directories

wowctl auto-detects your addon directory:

- **macOS**: `/Applications/World of Warcraft/_retail_/Interface/AddOns`
- **Windows**: `C:\Program Files (x86)\World of Warcraft\_retail_\Interface\AddOns`

You can override this in the config or with the `--addon-dir` flag.

## How It Works

### Addon Registry

wowctl maintains a local registry of managed addons. This tracks:
- Addon name, version, and source
- Which directories belong to each addon
- Dependency relationships

Addons installed by other tools are detected as "unmanaged" and shown separately.

### Dependency Management

When you install an addon, wowctl:
1. Queries CurseForge for the addon metadata
2. Resolves all required dependencies
3. Installs dependencies first, then the main addon
4. Tracks dependency relationships in the registry

When you remove an addon, wowctl:
1. Removes the addon directories
2. Checks for orphaned dependencies (no longer required by any addon)
3. Prompts you to remove orphans

### Atomic Operations

All installations and updates are atomic:
1. Download to temporary location
2. Extract to temporary directory
3. Validate (check for conflicts)
4. Move to addon directory
5. Update registry

If any step fails, the operation is rolled back and your addon directory remains unchanged.

## Troubleshooting

### Missing API Key

```
Error: CurseForge API key not found. Run 'wowctl config init' or set WOWCTL_CURSEFORGE_API_KEY environment variable
```

**Solution**: Run `wowctl config init` or set the `WOWCTL_CURSEFORGE_API_KEY` environment variable.

### Addon Directory Not Found

```
Error: Could not auto-detect WoW addon directory. Please set it manually with 'wowctl config set addon_dir <path>'
```

**Solution**: Set your addon directory manually:

```bash
wowctl config set addon_dir "/path/to/World of Warcraft/_retail_/Interface/AddOns"
```

### Network Errors

wowctl automatically retries failed requests up to 3 times with exponential backoff. If you continue to see network errors:
- Check your internet connection
- Verify your CurseForge API key is valid
- Check if CurseForge is experiencing issues

## Development

### Building

```bash
cargo build --release
```

### Running Tests

```bash
cargo test
```

### Logging

Enable verbose logging:

```bash
wowctl --verbose <command>
```

Or set the `RUST_LOG` environment variable:

```bash
RUST_LOG=debug wowctl <command>
```

## Roadmap

### Phase 1 (Current) — Core MVP
- ✅ Configuration management
- ✅ CurseForge integration
- ✅ Install, update, remove, list, search, info commands
- ✅ Automatic dependency resolution
- ✅ Orphaned dependency cleanup

### Phase 2 — Polish
- [ ] `adopt` command for managing existing addons
- [ ] Version pinning to prevent specific addons from updating
- [ ] Backup/restore of addon settings (SavedVariables)
- [ ] Export/import addon lists

### Phase 3 — Extensibility
- [ ] Additional addon sources (WoWInterface, Wago, GitHub)
- [ ] Plugin system for community-contributed sources
- [ ] JSON output mode for scripting
- [ ] WoW Classic support

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/)
- Addon data from [CurseForge](https://www.curseforge.com/)
