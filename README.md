# wowctl — World of Warcraft Addon Manager CLI

[![CI](https://github.com/thebigjc/wowctl/actions/workflows/ci.yml/badge.svg)](https://github.com/thebigjc/wowctl/actions/workflows/ci.yml)

wowctl is a command-line addon manager for World of Warcraft Retail. It's a
terminal alternative to GUI tools like the CurseForge app or WowUp — closer
in spirit to [instawow](https://github.com/layday/instawow) — for people who
would rather install, update, and manage addons from a shell than click
through a desktop app. It resolves dependencies automatically, tracks what
it installed in a local registry, and can pull addons from both CurseForge
and Wago Addons.

Written in Rust. Runs on macOS, Windows, and Linux/WSL (see the Linux note
below).

## Requirements / API keys

- **CurseForge (required)** — search, install, and update all use the
  CurseForge Core API. Keys are **not** self-serve: you apply via the form
  linked from
  [About the CurseForge API and How to Apply for a Key](https://support.curseforge.com/support/solutions/articles/9000208346-about-the-curseforge-api-and-how-to-apply-for-a-key),
  accept the 3rd-party API terms, and Overwolf reviews the request before
  issuing a key by email. Once you have it, set it during `wowctl config
  init` or via `WOWCTL_CURSEFORGE_API_KEY`. Note that the Core API key
  (`x-api-key`) is not the same as a legacy curseforge.com token.
- **Wago (optional)** — only needed for Wago-exclusive addons
  (`wago:<slug>` or an `addons.wago.io` URL). Wago has no public API; wowctl
  uses the same unofficial endpoint as other addon managers (see
  [`docs/adr/0001-wago-unofficial-external-api.md`](docs/adr/0001-wago-unofficial-external-api.md)).
  The access key is a benefit of the "Wago Addons Supporter" Patreon tier
  (~$3/month) at <https://addons.wago.io/patreon>. Without a key, Wago
  results are silently skipped and CurseForge-only usage works fine.

## Install

### Prebuilt binary

Download the latest release from the
[Releases page](https://github.com/thebigjc/wowctl/releases) (currently
v0.4.0):

| Platform | Artifact |
|---|---|
| macOS (Intel) | `wowctl-macos-x86_64.tar.gz` |
| macOS (Apple Silicon) | `wowctl-macos-aarch64.tar.gz` |
| Windows (x86_64) | `wowctl-windows-x86_64.zip` |

Extract the archive and put the `wowctl` (or `wowctl.exe`) binary on your
PATH.

There is no prebuilt Linux binary yet. Linux/WSL is supported in the code
(including auto-detecting a Windows WoW install under `/mnt/c/...` in WSL),
but you'll need to build from source.

### From source

```bash
git clone https://github.com/thebigjc/wowctl.git
cd wowctl
cargo build --release
```

The binary will be at `target/release/wowctl`. Add it to your PATH.

Or install directly with cargo:

```bash
cargo install --git https://github.com/thebigjc/wowctl
```

Requires Rust 1.85+ (edition 2024).

## Quick Start

### 1. Initial Setup

```bash
wowctl config init
```

This prompts for your CurseForge API key (verifying it against the API),
optionally your Wago access key, and auto-detects your WoW addon directory.

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

Dependencies are installed automatically.

### 4. List Installed Addons

```bash
wowctl list
```

### 5. Update Addons

```bash
wowctl update                    # check all addons, prompt before installing
wowctl update --auto             # install all updates without prompting
wowctl update deadly-boss-mods   # check/update one addon
```

### 6. Remove an Addon

```bash
wowctl remove deadly-boss-mods
```

Orphaned dependencies (no longer required by anything) are detected and you're prompted to remove them.

## Wago Addons support

wowctl can install and update addons from [Wago Addons](https://addons.wago.io)
alongside CurseForge:

    wowctl install wago:classcodex
    wowctl install https://addons.wago.io/addons/classcodex
    wowctl search classcodex --source wago

Bare slugs and CurseForge URLs keep meaning CurseForge; an explicit
`curseforge:` prefix is also accepted. Each addon remembers the source it was
installed from, and `wowctl update` checks each addon against its own source.

### Access key required

Wago has no public consumer API. wowctl uses the same unofficial API as other
addon managers (see `docs/adr/0001-wago-unofficial-external-api.md`), which
requires a **personal access key** from <https://addons.wago.io/patreon> —
the key is a benefit of the "Wago Addons Supporter" Patreon tier (~$3/month).

Set the key via the `WOWCTL_WAGO_ACCESS_KEY` environment variable or
`wowctl config set wago_access_key <key>` (env wins). Without a key, Wago is
simply skipped in merged search, and explicit `wago:` requests explain what's
missing.

> **Unofficial API**: the Wago endpoint is undocumented and may change or be
> revoked without notice. Wago support is best-effort.

## Commands

### `wowctl config`

Manage configuration.

- `wowctl config init` — Interactive first-time setup (CurseForge key, optional Wago key, addon directory)
- `wowctl config show` — Display current configuration
- `wowctl config set <key> <value>` — Set a configuration value

**Configuration Keys:**
- `addon_dir` — Path to WoW addon directory
- `curseforge_api_key` — Your CurseForge API key
- `wago_access_key` — Your Wago Addons personal access key (optional)
- `color` — Enable/disable colored output (`true` or `false`)
- `default_release_channel` (alias `channel`) — Default release channel: `stable`, `beta`, or `alpha`

### `wowctl search <query>`

Search for addons.

```bash
wowctl search "boss mods"
wowctl search "boss mods" --page 2
wowctl search classcodex --source wago       # limit to one source: curseforge or wago
```

### `wowctl install <addon>`

Install an addon with its dependencies.

```bash
wowctl install deadly-boss-mods
wowctl install https://www.curseforge.com/wow/addons/deadly-boss-mods
wowctl install wago:classcodex
wowctl install deadly-boss-mods --channel beta   # stable, beta, or alpha
```

### `wowctl update [addon]`

Check for and install updates.

```bash
wowctl update                    # check all addons
wowctl update deadly-boss-mods   # check specific addon
wowctl update --auto             # install all updates without prompting
wowctl update --auto-only        # only update addons with auto-update enabled (implies --auto)
wowctl update --channel beta     # override release channel for this run
```

### `wowctl remove <addon>`

Remove an installed addon.

```bash
wowctl remove deadly-boss-mods
```

### `wowctl list`

List addons in your addon directory.

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

### `wowctl adopt [addon_folder]`

Bring an already-installed, unmanaged addon folder under wowctl's registry
without reinstalling it.

```bash
wowctl adopt DBM-Core                       # adopt one folder, auto-detect the addon
wowctl adopt DBM-Core --slug deadly-boss-mods   # adopt with an explicit CurseForge slug
wowctl adopt --all                          # adopt every unmanaged addon at once
```

### `wowctl ignore <addon>` / `wowctl unignore <addon>`

Skip (or stop skipping) an addon during update checks.

```bash
wowctl ignore some-addon
wowctl unignore some-addon
```

### `wowctl auto-update <addon>` / `wowctl no-auto-update <addon>`

Enable or disable automatic updates for a specific addon (used together with
`wowctl update --auto-only`).

```bash
wowctl auto-update some-addon
wowctl no-auto-update some-addon
```

### `wowctl stale`

Find addons that haven't been updated in a while, and optionally remove them.

```bash
wowctl stale                # default threshold: 3 months
wowctl stale --months 6
```

## Global Flags

- `--no-color` — Disable colored output
- `--verbose` — Enable verbose logging (info level)
- `--debug` — Enable debug logging (debug level)
- `--addon-dir <path>` — Override the addon directory for this command
- `--help` — Show help
- `--version` — Show version

## Configuration

### Config File Location

- **macOS**: `~/Library/Application Support/wowctl/config.toml`
- **Windows**: `%APPDATA%\wowctl\config.toml`
- **Linux**: `~/.config/wowctl/config.toml`

### Data Directory

- **macOS**: `~/Library/Application Support/wowctl/`
- **Windows**: `%LOCALAPPDATA%\wowctl\`
- **Linux**: `~/.local/share/wowctl/`

The data directory contains the addon registry (`registry.toml`) which tracks all managed addons.

### Environment Variables

- `WOWCTL_CURSEFORGE_API_KEY` — CurseForge API key (takes precedence over config file)
- `WOWCTL_WAGO_ACCESS_KEY` — Wago Addons access key (takes precedence over config file)
- `NO_COLOR` — Disable colored output (respects standard)
- `RUST_LOG` — Control logging verbosity (e.g., `RUST_LOG=debug wowctl search foo`)

## Default Addon Directories

wowctl auto-detects your addon directory:

- **macOS**: `/Applications/World of Warcraft/_retail_/Interface/AddOns`
- **Windows**: `C:\Program Files (x86)\World of Warcraft\_retail_\Interface\AddOns`
- **Linux (WSL)**: `/mnt/c/Program Files (x86)/World of Warcraft/_retail_/Interface/AddOns`

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
1. Queries the addon's source (CurseForge or Wago) for metadata
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

```bash
wowctl --verbose <command>   # info-level logging
wowctl --debug <command>     # debug-level logging
```

Or set the `RUST_LOG` environment variable directly:

```bash
RUST_LOG=debug wowctl <command>
```

## Roadmap

### Done
- Configuration management
- CurseForge integration
- Wago Addons integration (search, install, update)
- Install, update, remove, list, search, info, adopt commands
- Automatic dependency resolution
- Orphaned dependency cleanup
- Ignore/unignore and per-addon auto-update toggles
- Stale-addon detection (`wowctl stale`)

### Open
- [ ] Version pinning to prevent specific addons from updating
- [ ] Backup/restore of addon settings (SavedVariables)
- [ ] Export/import addon lists
- [ ] Additional addon sources (WoWInterface, GitHub releases)
- [ ] Plugin system for community-contributed sources
- [ ] JSON output mode for scripting
- [ ] WoW Classic support
- [ ] Prebuilt Linux binary

## License

MIT License - see [LICENSE](LICENSE) file for details.

## Contributing

Contributions are welcome! See [CONTRIBUTING.md](CONTRIBUTING.md). For
background on the project's domain vocabulary, architecture, and API
integration notes, see [`CONTEXT.md`](CONTEXT.md), [`AGENTS.md`](AGENTS.md),
and the design decisions recorded in [`docs/adr/`](docs/adr/).

## Acknowledgments

- Built with [Rust](https://www.rust-lang.org/)
- Addon data from [CurseForge](https://www.curseforge.com/) and [Wago Addons](https://addons.wago.io)
