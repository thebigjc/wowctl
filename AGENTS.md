# AGENTS.md — wowctl

## Build & Run

- **Language:** Rust (edition 2024)
- **Build:** `cargo build --release`
- **Quick check:** `cargo check`
- **Binary location:** `target/release/wowctl`
- **Sandbox builds:** When building inside Cursor's sandbox, cargo may write to a sandboxed target directory. Use `CARGO_TARGET_DIR=target cargo build --release` to ensure the binary lands in the local `target/` directory.

## Project Structure

```
src/
  main.rs              # CLI entry point (clap derive)
  lib.rs               # Library root, module exports
  config.rs            # Config loading/saving, addon dir detection
  registry.rs          # Local addon registry (registry.toml)
  addon.rs             # Core data structures (AddonInfo, InstalledAddon, etc.)
  error.rs             # Error types (thiserror)
  circuit_breaker.rs   # Circuit breaker (Closed/Open/HalfOpen) for API resilience
  fingerprint.rs       # CurseForge-compatible MurmurHash2 fingerprinting
  utils.rs             # Zip extraction, .toc parsing, disk space, helpers
  colors.rs            # Colored output with global enable/disable
  commands/
    config.rs          # config init/show/set
    search.rs          # CurseForge search
    install.rs         # Install with dependency resolution
    update.rs          # Update all or specific addons
    remove.rs          # Remove with orphan cleanup
    list.rs            # List managed/unmanaged addons
    info.rs            # Show addon details
    adopt.rs           # Adopt unmanaged addons into registry
  sources/
    mod.rs             # AddonSource trait (extensible for future sources)
    curseforge.rs      # CurseForge API client
```

## Configuration

- **macOS config:** `~/Library/Application Support/wowctl/config.toml`
- **macOS data:** `~/Library/Application Support/wowctl/` (contains `registry.toml`)
- **Windows config:** `%APPDATA%\wowctl\config.toml`
- **Windows data:** `%LOCALAPPDATA%\wowctl\`
- The `dirs` crate handles platform paths — do not hardcode `~/.config/` on macOS.

## API Key

- CurseForge API key is required for all network operations (search, install, update, adopt).
- Set via `WOWCTL_CURSEFORGE_API_KEY` environment variable or `curseforge_api_key` in config.toml.
- Environment variable takes precedence over config file.
- Key is obtained through formal application at https://forms.monday.com/forms/dce5ccb7afda9a1c21dab1a1aa1d84eb?r=use1
- The legacy.curseforge.com token (X-Api-Token) is NOT the same as the Core API key (x-api-key).

## CurseForge API Constraints (from T&C)

- **No caching:** Do not save or cache any data obtained through the API.
- **No competing:** Do not build a product that competes directly with the CurseForge app.
- **Key is confidential:** Do not embed the API key in public source code.
- **No VPN/proxy access:** Do not conceal identity when accessing the API.
- **Usage quotas:** High usage may trigger a paid licensing requirement.

## CurseForge API Details

- **Game ID:** WoW = `1`
- **Addons class ID:** `1` (used in `classId` search param)
- **Retail gameVersionTypeId:** `517` — use this to filter files for WoW Retail
- **Classic gameVersionTypeId:** `67408` — WoW Classic
- **Slug lookup:** Use the `slug` query parameter on `/mods/search` for exact matching (not `searchFilter`, which is fuzzy text search)
- **Retail file filtering:** Pass `gameVersionTypeId=517` to `/mods/{id}/files` for server-side filtering — do not attempt client-side version string matching
- **Download URL fallback:** When `allowModDistribution` is not explicitly `true` on a mod, the `downloadUrl` field in file listings is `null`. Our fallback chain: (1) try the dedicated `GET /v1/mods/{modId}/files/{fileId}/download-url` endpoint, (2) if that also fails, construct a CDN URL via `https://edge.forgecdn.net/files/{id/1000}/{id%1000}/{fileName}`. The CDN hosts the same files the API would link to; the `id / 1000` and `id % 1000` path segments match the URL format the API returns when `downloadUrl` is populated.

## Known Issues

- **Version extraction heuristic:** `extract_version_from_display_name()` scans for the first token that looks like a version (starts with digit or `v`+digit) and returns everything from that point on. Falls back to everything after the first word. Addons with no version-like tokens in their display name (e.g. "CityGuide CityGuide.zip") will still show the non-version portion.

## Reference: WoWUp CurseForge Provider

WoWUp ([github.com/WowUp/WowUp](https://github.com/WowUp/WowUp)) is an open-source WoW addon updater with a mature CurseForge integration. Its CurseForge provider lives at `wowup-electron/src/app/addon-providers/curse-addon-provider.ts` and is a useful reference for working API best practices:

- **Version display:** WoWUp uses the CurseForge file `displayName` as-is for the version string (no parsing). This avoids fragile extraction but includes the addon name redundantly (e.g. "Plumber 1.8.8 b"). Our heuristic strips the addon-name prefix for cleaner CLI output.
- **Fingerprint scanning:** WoWUp uses the `getFingerprintMatches` API to match installed files to CurseForge entries by file hash, which is much more reliable than .toc metadata parsing for addon identification.
- **Batch fetching:** WoWUp uses the `getMods` POST endpoint with an array of mod IDs to fetch multiple addons in one request, reducing API calls during bulk operations.
- **File modules for folder names:** Each CurseForge file has a `modules` array whose `name` fields correspond to the top-level directories in the zip — useful for tracking which folders belong to an addon.
- **sortableGameVersions:** WoWUp filters files by `gameVersionTypeId` within `sortableGameVersions` for client-type compatibility, same approach we use with the query parameter.
- **Circuit breaker:** WoWUp wraps API calls in a circuit breaker for resilience against sustained failures.

## Development Workflow

- **Test-driven development:** Write test cases before fixing a bug or implementing a feature. Make the test fail first, then change the code until the test passes. Run `cargo test` to execute the test suite.
- **Keep AGENTS.md current:** Update this file whenever you learn something new about the project, its dependencies, APIs, or constraints. This is a living document.

## Testing Notes

- Run tests with `cargo test`.
- Interactive prompts (dialoguer) require a real TTY — piped stdin won't work.
- Use `--verbose` flag for debug-level tracing output.
- Use `--addon-dir <path>` to override the addon directory for testing.
- Use `--no-color` or `NO_COLOR=1` to disable colored output.

## Key Dependencies

- `clap` — CLI argument parsing (derive macros)
- `reqwest` — HTTP client (async, JSON, streaming)
- `tokio` — Async runtime
- `serde` / `serde_json` / `toml` — Serialization
- `zip` — Addon zip extraction
- `dialoguer` — Interactive terminal prompts
- `owo-colors` — Terminal color output
- `tracing` / `tracing-subscriber` — Structured logging (not `log` crate)
- `thiserror` — Error type derivation
- `dirs` — Platform-specific directory resolution
- `fs2` — Disk space checking
- `uuid` — Temp directory naming
