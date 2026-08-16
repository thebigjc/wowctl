# Wago Addons Source Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add Wago Addons (addons.wago.io) as a second first-class Source alongside CurseForge, so Wago-exclusive addons (motivating case: ClassCodex) can be searched, installed, and updated. Implements GitHub issue #8.

**Architecture:** The existing `AddonSource` trait is widened with the operations `update` needs (batch latest-version check, fetch-by-Addon-ID), and a new `WagoSource` client implements it against Wago's undocumented external API (`addons.wago.io/api/external`, per ADR-0001, reference implementation: WowUp's `wago-addon-provider.ts`). Because the trait's async style (RPIT-in-trait) is not dyn-compatible, commands dispatch through a small `AnySource` enum wrapping the concrete clients. Source selection is a pure parsing function (`wago:` / `curseforge:` prefixes, page URLs, bare slug → CurseForge). The Registry stays keyed by bare Slug; the existing per-addon `source` string field becomes authoritative for update dispatch.

**Tech Stack:** Rust (edition 2024), reqwest, tokio, serde, clap, thiserror, tracing. New dev-dependency: `wiremock` (HTTP-boundary tests).

## Global Constraints

- Rust edition 2024, rust-version floor 1.85 (`Cargo.toml` — do not change).
- `cargo clippy -- -D warnings` must pass before every commit (pre-push hook enforces it).
- Run tests with `cargo test`. TDD: write the failing test first, watch it fail, then implement.
- **No embedded Wago key in release builds** (ADR-0001). Credential precedence: `WOWCTL_WAGO_ACCESS_KEY` env var > `wago_access_key` config field. Missing key = Wago is "unconfigured", never a crash.
- Every Wago API call **and every Wago download** carries `Authorization: Bearer <access key>`.
- Respect `is_hidden_from_external`: such addons must be excluded from search/install/update.
- Wago calls pin `game_version=retail` (Flavor stays Retail-only).
- Wago stability tiers map 1:1 onto `ReleaseChannel`: stable→Stable, beta→Beta, alpha→Alpha.
- Source string literals in the Registry: `"curseforge"` and `"wago"` exactly.
- `adopt` remains CurseForge-only and keeps using `CurseForgeSource` directly (out of scope).
- Wago dependency resolution returns no dependencies.
- No cross-Source dedup in merged search; output groups rows by Source.
- Use `tracing` macros (`debug!`, `warn!`) for logging, never `println!` for diagnostics.
- **Wago API field names come from WowUp's provider and are unverified against live** — Task 14 (live acceptance) validates them; wiremock fixtures pin our assumed contract until then.

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `src/sources/mod.rs` | Modify | `AddonSource` trait (widened), `SourceKind`, `parse_addon_spec`, `AnySource` enum, `build_source`, shared `download_zip` helper, `BatchVersionCheck` (moved here) |
| `src/sources/wago.rs` | Create | Wago API client: serde models, pure mapping helpers, HTTP methods, `AddonSource` impl |
| `src/sources/curseforge.rs` | Modify | Injectable base URL, trait-method overrides, delegate download to shared helper |
| `src/addon.rs` | Modify | `VersionInfo.file_id` → `Option<u32>`, new `external_release_id` on `VersionInfo` and `InstalledAddon` |
| `src/config.rs` | Modify | `wago_access_key` field, `get_wago_access_key()` |
| `src/error.rs` | Modify | `Unauthorized` variant; source-neutral circuit-breaker message |
| `src/commands/install.rs` | Modify | Spec parsing, enum dispatch, cross-Source collision error |
| `src/commands/update.rs` | Modify | Group-by-Source dispatch, per-Source skip, `is_update_available` helper |
| `src/commands/search.rs` | Modify | Merged multi-Source search, `--source` filter |
| `src/commands/info.rs` | Modify | Wago page URL |
| `src/commands/config.rs` | Modify | init prompt, show indicator, set key |
| `src/main.rs` | Modify | `--source` flag on `search` |
| `src/utils.rs` | Modify | Delete `extract_slug_from_url` (superseded by `parse_addon_spec`) |
| `tests/curseforge_source.rs` | Create | wiremock tests for the CurseForge client |
| `tests/wago_source.rs` | Create | wiremock tests for the Wago client |
| `tests/wago_live.rs` | Create | `#[ignore]`d live acceptance test (ClassCodex) |
| `Cargo.toml` | Modify | dev-dependency `wiremock` |
| `README.md`, `AGENTS.md`, `config.toml.example` | Modify | Wago docs |

**Task interface conventions used throughout:** `SourceKind::as_str()` returns `"curseforge"` / `"wago"`. `AnySource` implements `AddonSource`. All commands construct sources via `build_source(kind, &config) -> Result<AnySource>`.

---

### Task 1: `SourceKind` enum and `parse_addon_spec`

**Files:**
- Modify: `src/sources/mod.rs`
- Test: unit tests in `src/sources/mod.rs`

**Interfaces:**
- Consumes: `crate::error::{Result, WowctlError}` (existing).
- Produces (later tasks rely on these exact signatures):
  - `pub enum SourceKind { CurseForge, Wago }` — `Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum`; `pub fn as_str(self) -> &'static str`; `impl Display`; `impl FromStr<Err = String>`.
  - `pub fn parse_addon_spec(input: &str) -> Result<(SourceKind, String)>`

- [ ] **Step 1: Write the failing tests**

Append to `src/sources/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_slug_defaults_to_curseforge() {
        assert_eq!(
            parse_addon_spec("weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn wago_prefix_selects_wago() {
        assert_eq!(
            parse_addon_spec("wago:classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn curseforge_prefix_selects_curseforge() {
        assert_eq!(
            parse_addon_spec("curseforge:weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn prefix_is_case_insensitive() {
        assert_eq!(
            parse_addon_spec("WAGO:classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_selects_wago() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_with_trailing_slash() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex/").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn wago_url_with_query_string() {
        assert_eq!(
            parse_addon_spec("https://addons.wago.io/addons/classcodex?utm_source=x").unwrap(),
            (SourceKind::Wago, "classcodex".to_string())
        );
    }

    #[test]
    fn curseforge_url_selects_curseforge() {
        assert_eq!(
            parse_addon_spec("https://www.curseforge.com/wow/addons/weakauras-2").unwrap(),
            (SourceKind::CurseForge, "weakauras-2".to_string())
        );
    }

    #[test]
    fn curseforge_url_without_www() {
        assert_eq!(
            parse_addon_spec("https://curseforge.com/wow/addons/details").unwrap(),
            (SourceKind::CurseForge, "details".to_string())
        );
    }

    #[test]
    fn unknown_prefix_errors() {
        let err = parse_addon_spec("wowinterface:foo").unwrap_err();
        assert!(err.to_string().contains("unknown source"));
    }

    #[test]
    fn unknown_url_errors() {
        assert!(parse_addon_spec("https://example.com/addons/foo").is_err());
    }

    #[test]
    fn empty_slug_after_prefix_errors() {
        assert!(parse_addon_spec("wago:").is_err());
    }

    #[test]
    fn bare_wago_addons_url_root_errors() {
        assert!(parse_addon_spec("https://addons.wago.io/addons/").is_err());
    }

    #[test]
    fn empty_input_errors() {
        assert!(parse_addon_spec("").is_err());
        assert!(parse_addon_spec("   ").is_err());
    }

    #[test]
    fn source_kind_string_roundtrip() {
        assert_eq!(SourceKind::CurseForge.as_str(), "curseforge");
        assert_eq!(SourceKind::Wago.as_str(), "wago");
        assert_eq!("curseforge".parse::<SourceKind>().unwrap(), SourceKind::CurseForge);
        assert_eq!("wago".parse::<SourceKind>().unwrap(), SourceKind::Wago);
        assert_eq!("WAGO".parse::<SourceKind>().unwrap(), SourceKind::Wago);
        assert!("wowinterface".parse::<SourceKind>().is_err());
        assert_eq!(SourceKind::Wago.to_string(), "wago");
        assert_eq!(SourceKind::CurseForge.to_string(), "curseforge");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sources::tests -- --nocapture`
Expected: COMPILE ERROR — `SourceKind` and `parse_addon_spec` not defined.

- [ ] **Step 3: Implement `SourceKind` and `parse_addon_spec`**

Add to `src/sources/mod.rs` (below the existing `use` lines; add `use crate::error::WowctlError;`, `use std::fmt;`, `use std::str::FromStr;` to the imports):

```rust
/// The addon platforms wowctl can talk to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, clap::ValueEnum)]
pub enum SourceKind {
    #[value(name = "curseforge")]
    CurseForge,
    #[value(name = "wago")]
    Wago,
}

impl SourceKind {
    /// The canonical string stored in the registry's per-addon `source` field.
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::CurseForge => "curseforge",
            SourceKind::Wago => "wago",
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for SourceKind {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "curseforge" => Ok(Self::CurseForge),
            "wago" => Ok(Self::Wago),
            _ => Err(format!("unknown source '{s}' (expected: curseforge, wago)")),
        }
    }
}

/// Parses a user-supplied addon spec into (Source, Slug).
///
/// Accepted forms:
/// - `classcodex` — bare slug, defaults to CurseForge
/// - `wago:classcodex`, `curseforge:weakauras-2` — explicit source prefix
/// - `https://addons.wago.io/addons/classcodex` — Wago page URL
/// - `https://www.curseforge.com/wow/addons/weakauras-2` — CurseForge page URL
pub fn parse_addon_spec(input: &str) -> Result<(SourceKind, String)> {
    let input = input.trim();
    if input.is_empty() {
        return Err(WowctlError::Source("Empty addon spec".to_string()));
    }
    if input.starts_with("http://") || input.starts_with("https://") {
        return parse_addon_url(input);
    }
    if let Some((prefix, rest)) = input.split_once(':') {
        let kind: SourceKind = prefix.parse().map_err(WowctlError::Source)?;
        if rest.is_empty() {
            return Err(WowctlError::Source(format!(
                "Missing slug after '{prefix}:'"
            )));
        }
        return Ok((kind, rest.to_string()));
    }
    Ok((SourceKind::CurseForge, input.to_string()))
}

/// Extracts the path segment immediately after `marker`, stopping at
/// `/`, `?`, or `#`. Returns None if the marker is absent or the segment empty.
fn slug_after<'a>(url: &'a str, marker: &str) -> Option<&'a str> {
    let idx = url.find(marker)? + marker.len();
    let slug = url[idx..].split(['/', '?', '#']).next().unwrap_or("");
    (!slug.is_empty()).then_some(slug)
}

fn parse_addon_url(url: &str) -> Result<(SourceKind, String)> {
    if let Some(slug) = slug_after(url, "addons.wago.io/addons/") {
        return Ok((SourceKind::Wago, slug.to_string()));
    }
    if let Some(slug) = slug_after(url, "curseforge.com/wow/addons/") {
        return Ok((SourceKind::CurseForge, slug.to_string()));
    }
    Err(WowctlError::Source(format!(
        "Unrecognized addon URL: {url} (expected a curseforge.com or addons.wago.io addon page URL)"
    )))
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib sources::tests`
Expected: all Task 1 tests PASS.

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add src/sources/mod.rs
git commit -m "feat: add SourceKind enum and addon spec parsing (wago:/curseforge: prefixes, page URLs)"
```

---

### Task 2: Wago access key in Config

**Files:**
- Modify: `src/config.rs`
- Test: unit tests in `src/config.rs`

**Interfaces:**
- Produces:
  - `Config.wago_access_key: Option<String>` (serialized field `wago_access_key`)
  - `pub fn get_wago_access_key(&self) -> Option<String>` — env `WOWCTL_WAGO_ACCESS_KEY` > config; blank strings treated as unset; `None` means "Wago unconfigured" (never an error here).

- [ ] **Step 1: Write the failing tests**

Append to `src/config.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_wago_key_env_wins() {
        assert_eq!(
            resolve_wago_key(Some("env-key".into()), Some("cfg-key".into())),
            Some("env-key".to_string())
        );
    }

    #[test]
    fn resolve_wago_key_falls_back_to_config() {
        assert_eq!(
            resolve_wago_key(None, Some("cfg-key".into())),
            Some("cfg-key".to_string())
        );
    }

    #[test]
    fn resolve_wago_key_blank_env_falls_through() {
        assert_eq!(
            resolve_wago_key(Some("  ".into()), Some("cfg-key".into())),
            Some("cfg-key".to_string())
        );
    }

    #[test]
    fn resolve_wago_key_none_when_unset() {
        assert_eq!(resolve_wago_key(None, None), None);
        assert_eq!(resolve_wago_key(Some(String::new()), Some("  ".into())), None);
    }

    #[test]
    fn config_without_wago_key_parses() {
        // Backward compat: existing config files have no wago_access_key field.
        let config: Config = toml::from_str(
            r#"
            addon_dir = "/tmp/addons"
            curseforge_api_key = "cf-key"
            color = true
            "#,
        )
        .unwrap();
        assert_eq!(config.wago_access_key, None);
    }

    #[test]
    fn config_with_wago_key_parses() {
        let config: Config = toml::from_str(r#"wago_access_key = "abc123""#).unwrap();
        assert_eq!(config.wago_access_key, Some("abc123".to_string()));
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib config::tests`
Expected: COMPILE ERROR — `resolve_wago_key` and `wago_access_key` not defined.

- [ ] **Step 3: Implement**

In `src/config.rs`, add the field to `Config` (after `curseforge_api_key`):

```rust
    /// Wago Addons personal access key. Can also be set via WOWCTL_WAGO_ACCESS_KEY env var.
    /// Keys come from addons.wago.io/patreon (Wago Addons Supporter tier); see ADR-0001.
    #[serde(default)]
    pub wago_access_key: Option<String>,
```

Add `wago_access_key: None,` to the `Default` impl for `Config`.

Add below `get_api_key` in `impl Config`:

```rust
    /// Gets the Wago access key, if configured.
    /// Precedence: WOWCTL_WAGO_ACCESS_KEY env var > config file. There is no
    /// embedded fallback key — Wago keys are personal (ADR-0001). `None`
    /// means the Wago source is unconfigured.
    pub fn get_wago_access_key(&self) -> Option<String> {
        resolve_wago_key(
            std::env::var("WOWCTL_WAGO_ACCESS_KEY").ok(),
            self.wago_access_key.clone(),
        )
    }
```

Add as a free function at module level (above the tests):

```rust
/// Resolves a Wago access key from an env-var value and a config value.
/// Env takes precedence; blank/whitespace values are treated as unset.
fn resolve_wago_key(env_val: Option<String>, config_val: Option<String>) -> Option<String> {
    env_val
        .filter(|s| !s.trim().is_empty())
        .or(config_val)
        .filter(|s| !s.trim().is_empty())
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib config::tests`
Expected: all Task 2 tests PASS. Also run `cargo test` (full suite) — existing tests must still pass (the new field is `Option` + `serde(default)`, so old configs and `config show` keep working).

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add src/config.rs
git commit -m "feat: add wago_access_key config field with env-var precedence"
```

---

### Task 3: Release-identity fields (`file_id` → `Option<u32>`, `external_release_id`)

Wago has no numeric file IDs; its release identity is the `logical_timestamp` number, which exceeds `u32`. This task makes `VersionInfo.file_id` optional and adds an opaque string `external_release_id` to `VersionInfo`, `BatchVersionCheck`, and `InstalledAddon`, plus a pure `is_update_available` comparison used by `update`.

**Files:**
- Modify: `src/addon.rs` (VersionInfo, InstalledAddon)
- Modify: `src/sources/curseforge.rs` (BatchVersionCheck, `get_latest_version`, `get_latest_versions_batch`)
- Modify: `src/commands/install.rs:246` (installed_file_id assignment)
- Modify: `src/commands/update.rs` (has_update sites, `apply_update`, new helper + tests)
- Modify: `src/commands/adopt.rs:126,254,318` (Option adjustments)
- Modify: `src/commands/remove.rs`, `src/commands/stale.rs` (test fixtures gain the new field)
- Test: unit tests in `src/commands/update.rs` and `src/addon.rs`

**Interfaces:**
- Produces:
  - `VersionInfo { pub file_id: Option<u32>, pub external_release_id: Option<String>, ... }` (other fields unchanged)
  - `InstalledAddon { ..., #[serde(default)] pub external_release_id: Option<String> }`
  - `BatchVersionCheck { pub addon_id: String, pub file_id: Option<u32>, pub external_release_id: Option<String>, pub version: String, pub display_name: String, pub released_at: String }`
  - `impl BatchVersionCheck { pub fn from_version_info(addon_id: &str, v: &VersionInfo) -> Self }`
  - `fn is_update_available(installed: &InstalledAddon, latest: &BatchVersionCheck) -> bool` (private to `commands::update`)

- [ ] **Step 1: Write the failing tests**

Append to `src/addon.rs` tests module:

```rust
    #[test]
    fn installed_addon_external_release_id_roundtrip() {
        let toml_str = r#"
            name = "ClassCodex"
            slug = "classcodex"
            version = "1.2.0"
            source = "wago"
            addon_id = "rNkynwKa"
            directories = ["ClassCodex"]
            is_dependency = false
            required_by = []
            external_release_id = "1755100000000000"
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(
            addon.external_release_id,
            Some("1755100000000000".to_string())
        );
    }

    #[test]
    fn installed_addon_external_release_id_defaults_to_none() {
        let toml_str = r#"
            name = "Plumber"
            slug = "plumber"
            version = "1.8.8"
            source = "curseforge"
            addon_id = "12345"
            directories = ["Plumber"]
            is_dependency = false
            required_by = []
        "#;
        let addon: InstalledAddon = toml::from_str(toml_str).unwrap();
        assert_eq!(addon.external_release_id, None);
    }
```

Add a new tests module at the bottom of `src/commands/update.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::curseforge::BatchVersionCheck;

    fn make_installed(
        file_id: Option<u32>,
        release_id: Option<&str>,
        version: &str,
    ) -> InstalledAddon {
        InstalledAddon {
            name: "Test".to_string(),
            slug: "test".to_string(),
            version: version.to_string(),
            source: "curseforge".to_string(),
            addon_id: "1".to_string(),
            directories: vec![],
            is_dependency: false,
            required_by: vec![],
            installed_file_id: file_id,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: None,
            auto_update: None,
            external_release_id: release_id.map(String::from),
        }
    }

    fn make_check(
        file_id: Option<u32>,
        release_id: Option<&str>,
        version: &str,
    ) -> BatchVersionCheck {
        BatchVersionCheck {
            addon_id: "1".to_string(),
            file_id,
            external_release_id: release_id.map(String::from),
            version: version.to_string(),
            display_name: version.to_string(),
            released_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn update_detected_by_external_release_id() {
        let installed = make_installed(None, Some("100"), "1.0");
        let latest = make_check(None, Some("200"), "1.0");
        assert!(is_update_available(&installed, &latest));
    }

    #[test]
    fn no_update_when_external_release_id_matches() {
        let installed = make_installed(None, Some("100"), "1.0");
        // Version string differs but release id matches: id wins.
        let latest = make_check(None, Some("100"), "1.1");
        assert!(!is_update_available(&installed, &latest));
    }

    #[test]
    fn update_detected_by_file_id() {
        let installed = make_installed(Some(10), None, "1.0");
        let latest = make_check(Some(11), None, "1.0");
        assert!(is_update_available(&installed, &latest));
    }

    #[test]
    fn no_update_when_file_id_matches() {
        let installed = make_installed(Some(10), None, "1.0");
        let latest = make_check(Some(10), None, "1.1");
        assert!(!is_update_available(&installed, &latest));
    }

    #[test]
    fn falls_back_to_version_comparison() {
        let installed = make_installed(None, None, "1.0");
        assert!(is_update_available(&installed, &make_check(None, None, "1.1")));
        assert!(!is_update_available(&installed, &make_check(None, None, "1.0")));
    }

    #[test]
    fn mixed_identity_falls_back_to_version() {
        // Installed pre-dates file-id tracking, latest has one: only versions are comparable.
        let installed = make_installed(None, None, "1.0");
        let latest = make_check(Some(11), None, "1.0");
        assert!(!is_update_available(&installed, &latest));
    }

    #[test]
    fn batch_check_from_version_info() {
        let v = crate::addon::VersionInfo {
            file_id: None,
            external_release_id: Some("12345".to_string()),
            version: "2.0".to_string(),
            display_name: "2.0".to_string(),
            download_url: "https://example.com/a.zip".to_string(),
            file_name: "a.zip".to_string(),
            file_size: 0,
            game_versions: vec![],
            released_at: "2026-01-01T00:00:00Z".to_string(),
            dependencies: vec![],
            modules: vec![],
        };
        let check = BatchVersionCheck::from_version_info("rNkynwKa", &v);
        assert_eq!(check.addon_id, "rNkynwKa");
        assert_eq!(check.file_id, None);
        assert_eq!(check.external_release_id, Some("12345".to_string()));
        assert_eq!(check.version, "2.0");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib`
Expected: COMPILE ERROR — `external_release_id` fields, `is_update_available`, and `from_version_info` do not exist.

- [ ] **Step 3: Change the data structures**

In `src/addon.rs`, `VersionInfo`:

```rust
/// Version information for a specific addon release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionInfo {
    /// Source-assigned numeric file ID. Some for CurseForge; None for Sources
    /// (e.g. Wago) that do not use numeric file IDs.
    pub file_id: Option<u32>,
    /// Opaque per-release identity for Sources without numeric file IDs
    /// (Wago's logical_timestamp). Used for update detection.
    #[serde(default)]
    pub external_release_id: Option<String>,
    pub version: String,
    // ... all remaining fields unchanged ...
```

In `src/addon.rs`, `InstalledAddon`, after `auto_update`:

```rust
    /// Opaque release identity for Sources without numeric file IDs (Wago).
    #[serde(default)]
    pub external_release_id: Option<String>,
```

- [ ] **Step 4: Fix all compile sites (compile-driven)**

Run `cargo check` repeatedly; the full list of required fixes:

1. `src/sources/curseforge.rs` — `BatchVersionCheck` becomes:

```rust
/// Lightweight version info from a batch mod lookup, sufficient for update detection.
#[derive(Debug)]
pub struct BatchVersionCheck {
    pub addon_id: String,
    pub file_id: Option<u32>,
    pub external_release_id: Option<String>,
    pub version: String,
    pub display_name: String,
    pub released_at: String,
}

impl BatchVersionCheck {
    /// Builds a batch-check entry from a full VersionInfo.
    pub fn from_version_info(addon_id: &str, v: &crate::addon::VersionInfo) -> Self {
        Self {
            addon_id: addon_id.to_string(),
            file_id: v.file_id,
            external_release_id: v.external_release_id.clone(),
            version: v.version.clone(),
            display_name: v.display_name.clone(),
            released_at: v.released_at.clone(),
        }
    }
}
```

2. `src/sources/curseforge.rs` `get_latest_versions_batch` — in the `results.insert(...)` block: `file_id: Some(retail_file_id), external_release_id: None,`.

3. `src/sources/curseforge.rs` `get_latest_version` — in the returned `VersionInfo`: `file_id: Some(file_id), external_release_id: None,`. Note the local `let file_id = latest_file.id;` and the `resolve_download_url(addon_id, file_id, ...)` call keep using the plain `u32`.

4. `src/commands/install.rs` — in the `InstalledAddon` literal: `installed_file_id: downloaded.version_info.file_id,` (drop the `Some(...)` wrapper) and add `external_release_id: downloaded.version_info.external_release_id.clone(),`. Note: clone `external_release_id` BEFORE the line `version: downloaded.version_info.version` moves the struct field (order the field inits so moves come last, or bind clones first).

5. `src/commands/update.rs` — replace both `has_update` computations with the helper (see Step 5). In `apply_update`: `installed.installed_file_id = download.version_info.file_id;` and add `installed.external_release_id = download.version_info.external_release_id.clone();`.

6. `src/commands/update.rs` `refresh_stale_metadata` — `entry.installed_file_id = version_info.file_id;` and add `entry.external_release_id = version_info.external_release_id.clone();`.

7. `src/commands/adopt.rs:126` — `installed_file_id: version_info.file_id,` (drop `Some(...)`). Lines 254 and 318: `v.file_id` instead of `Some(v.file_id)` (the destructured tuple's `file_id` slot is already `Option<u32>`).

8. Test fixtures constructing `InstalledAddon` literals need `external_release_id: None,` added: `src/commands/remove.rs` (`make_addon`), `src/commands/stale.rs` (fixture near line 242), and any others `cargo check` flags.

- [ ] **Step 5: Add `is_update_available` and use it**

In `src/commands/update.rs` add (near the other free functions):

```rust
/// Determines whether a newer release is available by comparing the strongest
/// release identity both sides share: external release ID (Wago), then numeric
/// file ID (CurseForge), then the version string.
fn is_update_available(
    installed: &InstalledAddon,
    latest: &crate::sources::curseforge::BatchVersionCheck,
) -> bool {
    match (
        installed.external_release_id.as_deref(),
        latest.external_release_id.as_deref(),
    ) {
        (Some(a), Some(b)) => a != b,
        _ => match (installed.installed_file_id, latest.file_id) {
            (Some(a), Some(b)) => a != b,
            _ => installed.version != latest.version,
        },
    }
}
```

Replace the batch-path check (currently `let has_update = match installed.installed_file_id { ... }`) with:

```rust
                if let Some(check) = batch_map.get(&installed.addon_id) {
                    if is_update_available(installed, check) {
                        updates.push(UpdateInfo { /* unchanged fields */ });
                    }
```

Replace the sequential-path check inside `check_updates_sequential` with:

```rust
            Ok(version_info) => {
                let check = crate::sources::curseforge::BatchVersionCheck::from_version_info(
                    &installed.addon_id,
                    &version_info,
                );
                if is_update_available(installed, &check) {
                    updates.push(UpdateInfo {
                        slug: installed.slug.clone(),
                        name: installed.name.clone(),
                        current_version: installed.version.clone(),
                        new_version: check.version.clone(),
                        addon_id: installed.addon_id.clone(),
                        channel: addon_channel,
                    });
                }
            }
```

- [ ] **Step 6: Run the full suite**

Run: `cargo test`
Expected: PASS, including the new `update::tests` and `addon` roundtrip tests.

- [ ] **Step 7: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add -A src/
git commit -m "feat: optional file_id and external_release_id for source-agnostic update detection"
```

---

### Task 4: HTTP seam — injectable base URL + shared zip download + wiremock

**Files:**
- Modify: `Cargo.toml` (dev-dependency)
- Modify: `src/sources/curseforge.rs` (base URL field, download delegates to helper)
- Modify: `src/sources/mod.rs` (shared `download_zip`)
- Create: `tests/curseforge_source.rs`

**Interfaces:**
- Produces:
  - `CurseForgeSource::with_base_url(api_key: String, api_base: String) -> Result<Self>` (public; `new` keeps its signature and delegates with the production base `https://api.curseforge.com/v1`)
  - `pub(crate) async fn download_zip(request: reqwest::RequestBuilder, download_url: &str, destination: &Path) -> Result<PathBuf>` in `src/sources/mod.rs` — identical validation to today's CurseForge download (status check, HTML rejection, `PK\x03\x04` magic, atomic-ish write)

- [ ] **Step 1: Add wiremock dev-dependency**

In `Cargo.toml` under `[dev-dependencies]`:

```toml
wiremock = "0.6"
```

Run: `cargo check --tests` — must succeed (network fetch of the crate).

- [ ] **Step 2: Write the failing tests**

Create `tests/curseforge_source.rs`:

```rust
//! HTTP-boundary tests for the CurseForge client against a local wiremock server.

use wiremock::matchers::{header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowctl::addon::ReleaseChannel;
use wowctl::sources::AddonSource;
use wowctl::sources::curseforge::CurseForgeSource;

fn search_body() -> serde_json::Value {
    serde_json::json!({
        "data": [{
            "id": 65387,
            "name": "WeakAuras",
            "slug": "weakauras-2",
            "summary": "A powerful framework",
            "downloadCount": 1000000.0,
            "latestFiles": [],
            "links": {"websiteUrl": "https://www.curseforge.com/wow/addons/weakauras-2"},
            "latestFilesIndexes": []
        }],
        "pagination": {"index": 0, "pageSize": 20, "resultCount": 1, "totalCount": 1}
    })
}

fn files_body() -> serde_json::Value {
    serde_json::json!({
        "data": [{
            "id": 5877543,
            "displayName": "WeakAuras 5.12.8",
            "fileName": "WeakAuras-5.12.8.zip",
            "downloadUrl": "https://example.com/wa.zip",
            "fileLength": 500000,
            "gameVersions": ["11.1.0"],
            "dependencies": [],
            "fileDate": "2026-03-01T00:00:00Z",
            "releaseType": 1,
            "modules": [{"name": "WeakAuras", "fingerprint": 1}]
        }]
    })
}

#[tokio::test]
async fn search_hits_api_with_key_and_maps_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mods/search"))
        .and(header("x-api-key", "test-key"))
        .and(query_param("gameId", "1"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body()))
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let result = source.search("weakauras", None).await.unwrap();

    assert_eq!(result.addons.len(), 1);
    assert_eq!(result.addons[0].slug, "weakauras-2");
    assert_eq!(result.addons[0].source, "curseforge");
    assert_eq!(result.total_count, 1);
}

#[tokio::test]
async fn get_latest_version_picks_retail_file() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mods/65387/files"))
        .and(header("x-api-key", "test-key"))
        .and(query_param("gameVersionTypeId", "517"))
        .respond_with(ResponseTemplate::new(200).set_body_json(files_body()))
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let v = source
        .get_latest_version("65387", ReleaseChannel::Stable)
        .await
        .unwrap();

    assert_eq!(v.file_id, Some(5877543));
    assert_eq!(v.external_release_id, None);
    assert_eq!(v.version, "5.12.8");
    assert_eq!(v.download_url, "https://example.com/wa.zip");
    assert_eq!(v.modules, vec!["WeakAuras".to_string()]);
}

#[tokio::test]
async fn download_writes_valid_zip() {
    let server = MockServer::start().await;
    let zip_bytes: &[u8] = b"PK\x03\x04rest-of-zip-payload";
    Mock::given(method("GET"))
        .and(path("/files/addon.zip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/zip")
                .set_body_bytes(zip_bytes),
        )
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("addon.zip");
    let url = format!("{}/files/addon.zip", server.uri());

    let written = source.download(&url, &dest).await.unwrap();
    assert_eq!(written, dest);
    assert_eq!(std::fs::read(&dest).unwrap(), zip_bytes);
}

#[tokio::test]
async fn download_rejects_html_error_page() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/files/broken.zip"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_string("<html>error</html>"),
        )
        .mount(&server)
        .await;

    let source = CurseForgeSource::with_base_url("test-key".to_string(), server.uri()).unwrap();
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("broken.zip");
    let url = format!("{}/files/broken.zip", server.uri());

    assert!(source.download(&url, &dest).await.is_err());
    assert!(!dest.exists());
}
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --test curseforge_source`
Expected: COMPILE ERROR — `with_base_url` does not exist.

- [ ] **Step 4: Implement the base-URL seam**

In `src/sources/curseforge.rs`:

1. Add field `api_base: String` to `CurseForgeSource`.
2. Replace the constructor:

```rust
    /// Creates a new CurseForge source with the provided API key.
    pub fn new(api_key: String) -> Result<Self> {
        Self::with_base_url(api_key, CURSEFORGE_API_BASE.to_string())
    }

    /// Creates a CurseForge source pointed at a custom API base URL (tests).
    pub fn with_base_url(api_key: String, api_base: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent(format!("wowctl/{}", env!("WOWCTL_VERSION")))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| WowctlError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            api_key,
            api_base,
            circuit_breaker: CircuitBreaker::new(),
        })
    }
```

3. Replace every `format!("{CURSEFORGE_API_BASE}/...")` with `format!("{}/...", self.api_base)` — sites: `get_addon_by_id`, `get_addon_info_by_id`, `resolve_download_url`, `get_latest_versions_batch`, `get_addon_infos_batch`, `get_fingerprint_matches`, `search`, `get_latest_version`, `get_addon_by_slug`. Keep the `CURSEFORGE_API_BASE` const (used by `new`).

- [ ] **Step 5: Extract the shared `download_zip` helper**

In `src/sources/mod.rs` add (imports: `reqwest`, `tokio::io::AsyncWriteExt`, `tracing::debug`):

```rust
/// Downloads a zip file via the given prepared request, validating that the
/// response is a real zip archive (not an HTML error page) before writing it
/// to `destination`. Shared by all Sources so download quality is identical.
pub(crate) async fn download_zip(
    request: reqwest::RequestBuilder,
    download_url: &str,
    destination: &Path,
) -> Result<PathBuf> {
    use crate::error::WowctlError;
    use tokio::io::AsyncWriteExt;

    debug!("Downloading from: {}", download_url);

    let response = request
        .send()
        .await
        .map_err(|e| WowctlError::Network(format!("Failed to download addon: {e}")))?;

    let status = response.status();
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("(not set)")
        .to_string();

    debug!("Response: status={}, content-type={}", status, content_type);

    if !status.is_success() {
        return Err(WowctlError::Network(format!(
            "Download failed with status: {status}"
        )));
    }

    // Reject HTML error pages that CDNs sometimes serve with 200 OK
    if content_type.contains("text/html") || content_type.contains("text/plain") {
        return Err(WowctlError::Network(format!(
            "Server returned {content_type} instead of a zip file — the download URL may be invalid: {download_url}"
        )));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| WowctlError::Network(format!("Failed to read download: {e}")))?;

    debug!("Downloaded {} bytes", bytes.len());

    // Validate ZIP magic bytes (PK\x03\x04) before writing to disk
    if bytes.len() < 4 || &bytes[..4] != b"PK\x03\x04" {
        if bytes.len() < 1024 {
            debug!(
                "Response body for invalid zip (small, {} bytes): {:?}",
                bytes.len(),
                String::from_utf8_lossy(&bytes)
            );
        }
        return Err(WowctlError::Extraction(format!(
            "Downloaded file is not a valid zip archive (bad magic bytes). \
             Got {} bytes, first 4: {:02x?}. \
             The server may have returned an error page. URL: {}",
            bytes.len(),
            &bytes[..bytes.len().min(4)],
            download_url
        )));
    }

    if let Some(parent) = destination.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let mut file = tokio::fs::File::create(destination).await?;
    file.write_all(&bytes).await?;
    file.flush().await?;
    drop(file);

    debug!("Downloaded to: {}", destination.display());
    Ok(destination.to_path_buf())
}
```

Replace `CurseForgeSource::download`'s body (in the `impl AddonSource` block) with:

```rust
    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        crate::sources::download_zip(self.client.get(download_url), download_url, destination)
            .await
    }
```

Delete the now-unused debug logging of content-length/encoding and first/last bytes only if clippy flags dead code; otherwise keep `download_zip` as the single copy (the moved code above intentionally drops the first/last-16-bytes dump — it predates the magic-byte check and is redundant).

- [ ] **Step 6: Run tests**

Run: `cargo test --test curseforge_source && cargo test`
Expected: all PASS.

- [ ] **Step 7: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add Cargo.toml Cargo.lock src/sources/ tests/curseforge_source.rs
git commit -m "test: wiremock HTTP seam for CurseForge; shared zip download helper"
```

---

### Task 5: Widen the `AddonSource` trait; move `BatchVersionCheck`

**Files:**
- Modify: `src/sources/mod.rs` (trait methods, `BatchVersionCheck` moves here)
- Modify: `src/sources/curseforge.rs` (trait-impl overrides; re-export removal)
- Modify: `src/commands/update.rs`, `src/commands/adopt.rs`, `src/commands/install.rs` (import paths / trait usage)

**Interfaces:**
- Consumes: `BatchVersionCheck::from_version_info` (Task 3).
- Produces — the trait gains three methods (later tasks call them through the trait):

```rust
    /// Gets addon information by its Source-assigned Addon ID.
    fn get_addon_info_by_id(
        &self,
        addon_id: &str,
    ) -> impl std::future::Future<Output = Result<AddonInfo>> + Send;

    /// Batch check of the latest version for many addons, keyed by Addon ID.
    /// Sources with a batch endpoint should override; the default loops the
    /// single-addon check and skips (with a debug log) addons that fail.
    fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> impl std::future::Future<Output = Result<HashMap<String, BatchVersionCheck>>> + Send;

    /// Batch fetch of AddonInfo by Addon ID. Default loops get_addon_info_by_id.
    fn get_addon_infos_batch(
        &self,
        addon_ids: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<AddonInfo>>> + Send;
```

- `BatchVersionCheck` now lives at `crate::sources::BatchVersionCheck` (moved verbatim from `curseforge.rs`, including `from_version_info`).

- [ ] **Step 1: Move `BatchVersionCheck`**

Cut the `BatchVersionCheck` struct + impl from `src/sources/curseforge.rs` and paste into `src/sources/mod.rs` (below the `SourceKind` code; `mod.rs` needs `use std::collections::HashMap;` and already imports `VersionInfo` — change `from_version_info`'s parameter type to plain `&VersionInfo`). In `curseforge.rs` add `use crate::sources::BatchVersionCheck;`. In `src/commands/update.rs` change imports to `use crate::sources::BatchVersionCheck;` and update the two `crate::sources::curseforge::BatchVersionCheck` paths from Task 3 to `BatchVersionCheck`.

Run: `cargo test` — must still pass before continuing.

- [ ] **Step 2: Add the trait methods with defaults**

In `src/sources/mod.rs`, extend `trait AddonSource` with the three methods above. Default bodies:

```rust
    fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> impl std::future::Future<Output = Result<HashMap<String, BatchVersionCheck>>> + Send
    {
        async move {
            let mut results = HashMap::new();
            for id in addon_ids {
                match self.get_latest_version(id, channel).await {
                    Ok(v) => {
                        results.insert(
                            id.to_string(),
                            BatchVersionCheck::from_version_info(id, &v),
                        );
                    }
                    Err(e) => {
                        tracing::debug!("Batch version check failed for {}: {}", id, e);
                    }
                }
            }
            Ok(results)
        }
    }

    fn get_addon_infos_batch(
        &self,
        addon_ids: &[String],
    ) -> impl std::future::Future<Output = Result<Vec<AddonInfo>>> + Send {
        async move {
            let mut infos = Vec::new();
            for id in addon_ids {
                infos.push(self.get_addon_info_by_id(id).await?);
            }
            Ok(infos)
        }
    }
```

(`get_addon_info_by_id` has no default — every Source must implement it.)

- [ ] **Step 3: Move CurseForge's implementations into the trait impl**

In `src/sources/curseforge.rs`, move these three existing **inherent** methods into the `impl AddonSource for CurseForgeSource` block as overrides, keeping their bodies unchanged: `get_addon_info_by_id`, `get_latest_versions_batch`, `get_addon_infos_batch`. (The inherent `get_addon_by_id` raw-JSON method, `get_fingerprint_matches`, and `extract_version` stay inherent — `adopt` uses them and remains CurseForge-only.)

- [ ] **Step 4: Verify callers compile through the trait**

`src/commands/update.rs` (`source.get_latest_versions_batch(...)`), `src/commands/install.rs` (`source.get_addon_infos_batch(...)`), and `src/commands/adopt.rs` (`source.get_addon_info_by_id(...)`) all already import `use crate::sources::AddonSource;` — method resolution now goes through the trait.

Run: `cargo check`
Expected: compiles with no changes needed in the commands beyond imports.

- [ ] **Step 5: Run the full suite, clippy, commit**

```bash
cargo test
cargo clippy -- -D warnings
git add src/
git commit -m "refactor: widen AddonSource trait with batch and by-id lookups"
```

---

### Task 6: Wago models, pure mapping, and the `Unauthorized` error

**Files:**
- Create: `src/sources/wago.rs` (models + pure helpers only; HTTP comes in Task 7)
- Modify: `src/sources/mod.rs` (add `pub mod wago;`)
- Modify: `src/error.rs` (`Unauthorized` variant; source-neutral circuit-breaker message)
- Test: unit tests in `src/sources/wago.rs`

**Interfaces:**
- Consumes: `ReleaseChannel` ordering (`Stable < Beta < Alpha`), `AddonInfo`, `VersionInfo`.
- Produces (Task 7 builds on these):
  - serde models `WagoSearchResponse`, `WagoSearchItem`, `WagoAddonDetail`, `WagoReleases`, `WagoRelease`, `WagoRecentsRequest`, `WagoRecentsResponse`, `WagoRecentsEntry` (all private to the module)
  - `fn select_release(releases: &WagoReleases, channel: ReleaseChannel) -> Option<&WagoRelease>`
  - `fn stability_param(channel: ReleaseChannel) -> &'static str`
  - `fn item_slug(item: &WagoSearchItem) -> Option<String>`
  - `fn to_addon_info(item: &WagoSearchItem) -> AddonInfo` (source = `"wago"`)
  - `fn to_version_info(release: &WagoRelease, file_name: String) -> Result<VersionInfo>`
  - `WowctlError::Unauthorized(String)`

**Contract note:** field names follow WowUp's `wago-addon-provider.ts` (our reference implementation per ADR-0001): search returns `{"data": [...]}`; addon detail returns the object bare with `recent_releases` keyed by stability; `_recents` returns `{"addons": {"<id>": {...}}}`; releases carry `label`, `download_link`, `created_at`, `logical_timestamp`, `stability`, `supported_retail_patch`. Task 14 validates against the live API — if a name differs, fix the serde rename and the fixtures, nothing else.

- [ ] **Step 1: Add the error variant**

In `src/error.rs` add after `MissingApiKey`:

```rust
    #[error("Unauthorized: {0}")]
    Unauthorized(String),
```

And make the circuit-breaker message source-neutral (it now guards both APIs):

```rust
    #[error(
        "The addon source API is temporarily unavailable after multiple consecutive failures. Please try again in a few seconds."
    )]
    CircuitBreakerOpen,
```

Run: `cargo test` — still green (message text is asserted nowhere).

- [ ] **Step 2: Write the failing tests**

Create `src/sources/wago.rs`:

```rust
//! Wago Addons source implementation.
//!
//! Talks to the undocumented external API at addons.wago.io/api/external
//! using a personal access key (Bearer auth on every call, downloads
//! included). Reference implementation: WowUp's wago-addon-provider.ts.
//! See ADR-0001 for why this API and its constraints.

use crate::addon::{AddonInfo, ReleaseChannel, VersionInfo};
use crate::error::{Result, WowctlError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Models and helpers are added in the implementation steps below.

#[cfg(test)]
mod tests {
    use super::*;

    fn release(label: &str, ts: u64) -> WagoRelease {
        WagoRelease {
            label: label.to_string(),
            download_link: Some(format!("https://example.com/{label}.zip")),
            created_at: Some("2026-08-01T00:00:00Z".to_string()),
            logical_timestamp: Some(ts),
            stability: None,
            supported_retail_patch: None,
        }
    }

    #[test]
    fn stable_channel_only_sees_stable() {
        let releases = WagoReleases {
            stable: Some(release("1.0", 100)),
            beta: Some(release("2.0-beta", 200)),
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Stable).unwrap();
        assert_eq!(picked.label, "1.0");
    }

    #[test]
    fn beta_channel_picks_newest_of_stable_and_beta() {
        let releases = WagoReleases {
            stable: Some(release("1.0", 100)),
            beta: Some(release("2.0-beta", 200)),
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Beta).unwrap();
        assert_eq!(picked.label, "2.0-beta");
    }

    #[test]
    fn beta_channel_prefers_newer_stable() {
        let releases = WagoReleases {
            stable: Some(release("2.1", 400)),
            beta: Some(release("2.0-beta", 200)),
            alpha: None,
        };
        let picked = select_release(&releases, ReleaseChannel::Beta).unwrap();
        assert_eq!(picked.label, "2.1");
    }

    #[test]
    fn alpha_channel_sees_all_tiers() {
        let releases = WagoReleases {
            stable: None,
            beta: None,
            alpha: Some(release("3.0-alpha", 300)),
        };
        let picked = select_release(&releases, ReleaseChannel::Alpha).unwrap();
        assert_eq!(picked.label, "3.0-alpha");
    }

    #[test]
    fn no_eligible_release_returns_none() {
        let releases = WagoReleases {
            stable: None,
            beta: Some(release("2.0-beta", 200)),
            alpha: None,
        };
        assert!(select_release(&releases, ReleaseChannel::Stable).is_none());
    }

    #[test]
    fn stability_param_maps_channels() {
        assert_eq!(stability_param(ReleaseChannel::Stable), "stable");
        assert_eq!(stability_param(ReleaseChannel::Beta), "beta");
        assert_eq!(stability_param(ReleaseChannel::Alpha), "alpha");
    }

    #[test]
    fn item_slug_prefers_slug_field() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: Some("classcodex".to_string()),
            display_name: "ClassCodex".to_string(),
            summary: None,
            download_count: None,
            website_url: Some("https://addons.wago.io/addons/other".to_string()),
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        assert_eq!(item_slug(&item), Some("classcodex".to_string()));
    }

    #[test]
    fn item_slug_falls_back_to_website_url() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: None,
            display_name: "ClassCodex".to_string(),
            summary: None,
            download_count: None,
            website_url: Some("https://addons.wago.io/addons/classcodex/".to_string()),
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        assert_eq!(item_slug(&item), Some("classcodex".to_string()));
    }

    #[test]
    fn to_addon_info_maps_fields_and_tags_source() {
        let item = WagoSearchItem {
            id: "rNkynwKa".to_string(),
            slug: Some("classcodex".to_string()),
            display_name: "ClassCodex".to_string(),
            summary: Some("Class guide addon".to_string()),
            download_count: Some(1234.0),
            website_url: None,
            is_hidden_from_external: false,
            releases: WagoReleases::default(),
        };
        let info = to_addon_info(&item);
        assert_eq!(info.id, "rNkynwKa");
        assert_eq!(info.slug, "classcodex");
        assert_eq!(info.name, "ClassCodex");
        assert_eq!(info.description, Some("Class guide addon".to_string()));
        assert_eq!(info.download_count, Some(1234));
        assert_eq!(info.source, "wago");
    }

    #[test]
    fn to_version_info_maps_release() {
        let mut r = release("1.2.0", 1755100000000000);
        r.supported_retail_patch = Some("11.2.0".to_string());
        let v = to_version_info(&r, "classcodex.zip".to_string()).unwrap();
        assert_eq!(v.file_id, None);
        assert_eq!(v.external_release_id, Some("1755100000000000".to_string()));
        assert_eq!(v.version, "1.2.0");
        assert_eq!(v.display_name, "1.2.0");
        assert_eq!(v.download_url, "https://example.com/1.2.0.zip");
        assert_eq!(v.file_name, "classcodex.zip");
        assert_eq!(v.game_versions, vec!["11.2.0".to_string()]);
        assert_eq!(v.released_at, "2026-08-01T00:00:00Z");
        assert!(v.dependencies.is_empty());
        assert!(v.modules.is_empty());
    }

    #[test]
    fn to_version_info_without_download_link_errors() {
        let mut r = release("1.2.0", 1);
        r.download_link = None;
        assert!(to_version_info(&r, "x.zip".to_string()).is_err());
    }

    #[test]
    fn search_response_deserializes() {
        let json = serde_json::json!({
            "data": [{
                "id": "rNkynwKa",
                "slug": "classcodex",
                "display_name": "ClassCodex",
                "summary": "Class guide addon",
                "download_count": 1234,
                "website_url": "https://addons.wago.io/addons/classcodex",
                "releases": {
                    "stable": {
                        "label": "1.2.0",
                        "download_link": "https://addons.wago.io/api/external/files/abc/download",
                        "created_at": "2026-08-01T00:00:00Z",
                        "logical_timestamp": 1755100000000000u64
                    }
                }
            }]
        });
        let resp: WagoSearchResponse = serde_json::from_value(json).unwrap();
        assert_eq!(resp.data.len(), 1);
        assert_eq!(resp.data[0].id, "rNkynwKa");
        assert!(!resp.data[0].is_hidden_from_external);
    }

    #[test]
    fn detail_deserializes_with_recent_releases() {
        let json = serde_json::json!({
            "id": "rNkynwKa",
            "slug": "classcodex",
            "display_name": "ClassCodex",
            "summary": "Class guide addon",
            "download_count": 1234,
            "website_url": "https://addons.wago.io/addons/classcodex",
            "is_hidden_from_external": false,
            "recent_releases": {
                "stable": {
                    "label": "1.2.0",
                    "download_link": "https://addons.wago.io/api/external/files/abc/download",
                    "created_at": "2026-08-01T00:00:00Z",
                    "logical_timestamp": 1755100000000000u64,
                    "stability": "stable",
                    "supported_retail_patch": "11.2.0"
                }
            }
        });
        let detail: WagoAddonDetail = serde_json::from_value(json).unwrap();
        assert_eq!(detail.slug, "classcodex");
        assert_eq!(
            detail.recent_releases.stable.as_ref().unwrap().label,
            "1.2.0"
        );
    }

    #[test]
    fn recents_response_deserializes() {
        let json = serde_json::json!({
            "addons": {
                "rNkynwKa": {
                    "id": "rNkynwKa",
                    "recent_releases": {
                        "stable": {
                            "label": "1.3.0",
                            "download_link": "https://example.com/1.3.0.zip",
                            "created_at": "2026-08-10T00:00:00Z",
                            "logical_timestamp": 1755200000000000u64
                        }
                    }
                }
            }
        });
        let resp: WagoRecentsResponse = serde_json::from_value(json).unwrap();
        let entry = resp.addons.get("rNkynwKa").unwrap();
        assert_eq!(
            entry.recent_releases.stable.as_ref().unwrap().label,
            "1.3.0"
        );
    }

    #[test]
    fn recents_request_serializes() {
        let req = WagoRecentsRequest {
            game_version: "retail",
            addons: vec!["rNkynwKa".to_string(), "abc123".to_string()],
        };
        assert_eq!(
            serde_json::to_value(&req).unwrap(),
            serde_json::json!({"game_version": "retail", "addons": ["rNkynwKa", "abc123"]})
        );
    }

    #[test]
    fn hidden_flag_deserializes_true() {
        let json = serde_json::json!({
            "id": "x", "display_name": "Hidden", "is_hidden_from_external": true
        });
        let item: WagoSearchItem = serde_json::from_value(json).unwrap();
        assert!(item.is_hidden_from_external);
    }
}
```

Register the module in `src/sources/mod.rs`:

```rust
pub mod curseforge;
pub mod wago;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib sources::wago`
Expected: COMPILE ERROR — models and helpers not defined.

- [ ] **Step 4: Implement models and helpers**

Add to `src/sources/wago.rs` above the tests:

```rust
#[derive(Debug, Deserialize)]
struct WagoSearchResponse {
    #[serde(default)]
    data: Vec<WagoSearchItem>,
}

#[derive(Debug, Deserialize)]
struct WagoSearchItem {
    id: String,
    #[serde(default)]
    slug: Option<String>,
    display_name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    download_count: Option<f64>,
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    is_hidden_from_external: bool,
    #[serde(default)]
    releases: WagoReleases,
}

#[derive(Debug, Deserialize)]
struct WagoAddonDetail {
    id: String,
    slug: String,
    display_name: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    download_count: Option<f64>,
    #[serde(default)]
    website_url: Option<String>,
    #[serde(default)]
    is_hidden_from_external: bool,
    #[serde(default)]
    recent_releases: WagoReleases,
}

/// Releases keyed by Wago stability tier. Tiers map 1:1 onto ReleaseChannel.
#[derive(Debug, Default, Deserialize)]
struct WagoReleases {
    #[serde(default)]
    stable: Option<WagoRelease>,
    #[serde(default)]
    beta: Option<WagoRelease>,
    #[serde(default)]
    alpha: Option<WagoRelease>,
}

#[derive(Debug, Clone, Deserialize)]
struct WagoRelease {
    label: String,
    #[serde(default)]
    download_link: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    /// Wago's monotonically-increasing release marker; our external_release_id.
    #[serde(default)]
    logical_timestamp: Option<u64>,
    #[serde(default)]
    stability: Option<String>,
    #[serde(default)]
    supported_retail_patch: Option<String>,
}

#[derive(Debug, Serialize)]
struct WagoRecentsRequest<'a> {
    game_version: &'a str,
    addons: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct WagoRecentsResponse {
    #[serde(default)]
    addons: HashMap<String, WagoRecentsEntry>,
}

#[derive(Debug, Deserialize)]
struct WagoRecentsEntry {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    recent_releases: WagoReleases,
}

/// Maps a ReleaseChannel to Wago's stability query-parameter value.
fn stability_param(channel: ReleaseChannel) -> &'static str {
    match channel {
        ReleaseChannel::Stable => "stable",
        ReleaseChannel::Beta => "beta",
        ReleaseChannel::Alpha => "alpha",
    }
}

/// Picks the newest release the channel allows (stable ⊆ beta ⊆ alpha),
/// newest judged by logical_timestamp.
fn select_release(releases: &WagoReleases, channel: ReleaseChannel) -> Option<&WagoRelease> {
    let mut candidates: Vec<&WagoRelease> = Vec::new();
    if let Some(r) = &releases.stable {
        candidates.push(r);
    }
    if channel >= ReleaseChannel::Beta
        && let Some(r) = &releases.beta
    {
        candidates.push(r);
    }
    if channel >= ReleaseChannel::Alpha
        && let Some(r) = &releases.alpha
    {
        candidates.push(r);
    }
    candidates
        .into_iter()
        .max_by_key(|r| r.logical_timestamp.unwrap_or(0))
}

/// Resolves an item's Slug: explicit field first, else the last path segment
/// of its website URL.
fn item_slug(item: &WagoSearchItem) -> Option<String> {
    if let Some(s) = &item.slug {
        return Some(s.clone());
    }
    item.website_url
        .as_deref()
        .and_then(|u| u.trim_end_matches('/').rsplit('/').next())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

fn to_addon_info(item: &WagoSearchItem) -> AddonInfo {
    AddonInfo {
        id: item.id.clone(),
        name: item.display_name.clone(),
        slug: item_slug(item).unwrap_or_else(|| item.id.clone()),
        description: item.summary.clone(),
        download_count: item.download_count.map(|d| d as u64),
        source: "wago".to_string(),
    }
}

fn to_version_info(release: &WagoRelease, file_name: String) -> Result<VersionInfo> {
    let download_url = release.download_link.clone().ok_or_else(|| {
        WowctlError::Source(format!(
            "Wago release '{}' has no download link",
            release.label
        ))
    })?;
    Ok(VersionInfo {
        file_id: None,
        external_release_id: release.logical_timestamp.map(|t| t.to_string()),
        version: release.label.clone(),
        display_name: release.label.clone(),
        download_url,
        file_name,
        // Wago does not report file sizes; 0 makes the disk-space check a no-op.
        file_size: 0,
        game_versions: release
            .supported_retail_patch
            .clone()
            .map(|p| vec![p])
            .unwrap_or_default(),
        released_at: release.created_at.clone().unwrap_or_default(),
        dependencies: vec![],
        modules: vec![],
    })
}
```

Note: `#[allow(dead_code)]` may be needed on `stability` and `WagoRecentsEntry.id` until Task 7 (clippy `-D warnings` treats dead code as an error only via `dead_code` lint — add `#[allow(dead_code)]` on those two fields now and remove it in Task 7 if they become used, or keep it: they exist to document the wire format like `CfPagination`'s unused fields do).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test --lib sources::wago`
Expected: all Task 6 tests PASS.

- [ ] **Step 6: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add src/sources/ src/error.rs
git commit -m "feat: Wago API models, release selection, and Unauthorized error"
```

---

### Task 7: Wago HTTP client and `AddonSource` implementation

**Files:**
- Modify: `src/sources/wago.rs`
- Create: `tests/wago_source.rs`

**Interfaces:**
- Consumes: `download_zip` (Task 4), trait defaults (Task 5), models/helpers (Task 6), `CircuitBreaker` (existing).
- Produces:
  - `pub struct WagoSource` with `pub fn new(access_key: String) -> Result<Self>` and `pub fn with_base_url(access_key: String, api_base: String) -> Result<Self>` (production base: `https://addons.wago.io/api/external`)
  - `impl AddonSource for WagoSource` — all five original methods plus overrides for `get_addon_info_by_id` and `get_latest_versions_batch` (`get_addon_infos_batch` uses the trait default)
  - Every request and download sends `Authorization: Bearer <access key>`; 401 → `WowctlError::Unauthorized` with Patreon guidance; 404 → `AddonNotFound`; 429 → retry with backoff; circuit breaker wraps all calls.

- [ ] **Step 1: Write the failing tests**

Create `tests/wago_source.rs`:

```rust
//! HTTP-boundary tests for the Wago client against a local wiremock server.
//! The canned JSON pins the API contract we assume (derived from WowUp's
//! wago-addon-provider.ts); tests/wago_live.rs validates it against the
//! real API.

use wiremock::matchers::{body_json, header, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};
use wowctl::addon::ReleaseChannel;
use wowctl::error::WowctlError;
use wowctl::sources::AddonSource;
use wowctl::sources::wago::WagoSource;

const KEY: &str = "test-wago-key";

fn source(server: &MockServer) -> WagoSource {
    WagoSource::with_base_url(KEY.to_string(), server.uri()).unwrap()
}

fn stable_release(label: &str, ts: u64, url: &str) -> serde_json::Value {
    serde_json::json!({
        "label": label,
        "download_link": url,
        "created_at": "2026-08-01T00:00:00Z",
        "logical_timestamp": ts,
        "stability": "stable",
        "supported_retail_patch": "11.2.0"
    })
}

fn search_body(server_uri: &str) -> serde_json::Value {
    serde_json::json!({
        "data": [
            {
                "id": "rNkynwKa",
                "slug": "classcodex",
                "display_name": "ClassCodex",
                "summary": "Class guide addon",
                "download_count": 1234,
                "website_url": "https://addons.wago.io/addons/classcodex",
                "releases": {
                    "stable": stable_release("1.2.0", 100, &format!("{server_uri}/dl/classcodex"))
                }
            },
            {
                "id": "hidden01",
                "slug": "hidden-addon",
                "display_name": "HiddenAddon",
                "is_hidden_from_external": true,
                "releases": {}
            }
        ]
    })
}

fn detail_body(server_uri: &str) -> serde_json::Value {
    serde_json::json!({
        "id": "rNkynwKa",
        "slug": "classcodex",
        "display_name": "ClassCodex",
        "summary": "Class guide addon",
        "download_count": 1234,
        "website_url": "https://addons.wago.io/addons/classcodex",
        "is_hidden_from_external": false,
        "recent_releases": {
            "stable": stable_release("1.2.0", 1755100000000000, &format!("{server_uri}/dl/classcodex")),
            "beta": stable_release("1.3.0-beta", 1755200000000000, &format!("{server_uri}/dl/classcodex-beta"))
        }
    })
}

#[tokio::test]
async fn search_sends_bearer_and_filters_hidden() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(query_param("query", "classcodex"))
        .and(query_param("game_version", "retail"))
        .and(query_param("stability", "stable"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body(&server.uri())))
        .mount(&server)
        .await;

    let result = source(&server).search("classcodex", None).await.unwrap();

    // The hidden addon must be excluded (is_hidden_from_external).
    assert_eq!(result.addons.len(), 1);
    assert_eq!(result.addons[0].slug, "classcodex");
    assert_eq!(result.addons[0].source, "wago");
}

#[tokio::test]
async fn get_latest_version_stable_channel() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(query_param("game_version", "retail"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let v = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap();

    assert_eq!(v.version, "1.2.0");
    assert_eq!(v.file_id, None);
    assert_eq!(v.external_release_id, Some("1755100000000000".to_string()));
    assert!(v.download_url.ends_with("/dl/classcodex"));
    assert!(v.dependencies.is_empty());
}

#[tokio::test]
async fn get_latest_version_beta_channel_picks_newer_beta() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let v = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Beta)
        .await
        .unwrap();

    assert_eq!(v.version, "1.3.0-beta");
}

#[tokio::test]
async fn hidden_addon_detail_is_not_found() {
    let server = MockServer::start().await;
    let mut body = detail_body(&server.uri());
    body["is_hidden_from_external"] = serde_json::json!(true);
    Mock::given(method("GET"))
        .and(path("/addons/rNkynwKa"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let err = source(&server)
        .get_latest_version("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap_err();
    assert!(matches!(err, WowctlError::AddonNotFound(_)));
}

#[tokio::test]
async fn unauthorized_maps_to_helpful_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(401))
        .mount(&server)
        .await;

    let err = source(&server).search("anything", None).await.unwrap_err();
    match err {
        WowctlError::Unauthorized(msg) => {
            assert!(msg.contains("addons.wago.io/patreon"));
            assert!(msg.contains("Wago Addons Supporter"));
        }
        other => panic!("expected Unauthorized, got: {other}"),
    }
}

#[tokio::test]
async fn get_addon_by_slug_matches_exact_slug() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .and(query_param("query", "classcodex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(search_body(&server.uri())))
        .mount(&server)
        .await;

    let info = source(&server).get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
    assert_eq!(info.slug, "classcodex");
}

#[tokio::test]
async fn get_addon_by_slug_falls_back_to_detail_lookup() {
    let server = MockServer::start().await;
    // Search misses...
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&server)
        .await;
    // ...but the detail endpoint resolves the slug directly.
    Mock::given(method("GET"))
        .and(path("/addons/classcodex"))
        .respond_with(ResponseTemplate::new(200).set_body_json(detail_body(&server.uri())))
        .mount(&server)
        .await;

    let info = source(&server).get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
}

#[tokio::test]
async fn get_addon_by_slug_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/addons/_search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/addons/nope"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = source(&server).get_addon_by_slug("nope").await.unwrap_err();
    assert!(matches!(err, WowctlError::AddonNotFound(_)));
}

#[tokio::test]
async fn recents_batch_maps_to_version_checks() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/addons/_recents"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .and(body_json(serde_json::json!({
            "game_version": "retail",
            "addons": ["rNkynwKa"]
        })))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "addons": {
                "rNkynwKa": {
                    "id": "rNkynwKa",
                    "recent_releases": {
                        "stable": stable_release("1.3.0", 1755300000000000u64, "https://example.com/1.3.0.zip")
                    }
                }
            }
        })))
        .mount(&server)
        .await;

    let checks = source(&server)
        .get_latest_versions_batch(&["rNkynwKa"], ReleaseChannel::Stable)
        .await
        .unwrap();

    let check = checks.get("rNkynwKa").unwrap();
    assert_eq!(check.version, "1.3.0");
    assert_eq!(check.file_id, None);
    assert_eq!(check.external_release_id, Some("1755300000000000".to_string()));
}

#[tokio::test]
async fn download_sends_bearer_and_validates_zip() {
    let server = MockServer::start().await;
    let zip_bytes: &[u8] = b"PK\x03\x04wago-zip-payload";
    Mock::given(method("GET"))
        .and(path("/dl/classcodex"))
        .and(header("authorization", format!("Bearer {KEY}").as_str()))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "application/zip")
                .set_body_bytes(zip_bytes),
        )
        .mount(&server)
        .await;

    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("classcodex.zip");
    let url = format!("{}/dl/classcodex", server.uri());

    source(&server).download(&url, &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), zip_bytes);
}

#[tokio::test]
async fn resolve_dependencies_is_empty() {
    let server = MockServer::start().await;
    let deps = source(&server)
        .resolve_dependencies("rNkynwKa", ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(deps.is_empty());
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --test wago_source`
Expected: COMPILE ERROR — `WagoSource` not defined.

- [ ] **Step 3: Implement the client**

Add to `src/sources/wago.rs` (above the models; extend imports with `use crate::circuit_breaker::CircuitBreaker;`, `use crate::sources::{AddonSource, BatchVersionCheck, SearchResult-related types};`, `use reqwest::Client;`, `use serde::de::DeserializeOwned;`, `use std::path::{Path, PathBuf};`, `use std::time::Duration;`, `use tracing::{debug, warn};` — plus `SearchResult` from `crate::addon`):

```rust
const WAGO_API_BASE: &str = "https://addons.wago.io/api/external";
const GAME_VERSION_RETAIL: &str = "retail";
const HTTP_TIMEOUT_SECS: u64 = 60;
const MAX_RETRIES: u32 = 3;
const INITIAL_BACKOFF_MS: u64 = 1000;

/// Wago Addons source implementation. Every call carries Bearer auth.
pub struct WagoSource {
    client: Client,
    access_key: String,
    api_base: String,
    circuit_breaker: CircuitBreaker,
}

/// The 401 error users see; points at where keys come from (ADR-0001).
fn unauthorized_error() -> WowctlError {
    WowctlError::Unauthorized(
        "Wago rejected the access key (HTTP 401). Personal access keys come from \
         https://addons.wago.io/patreon and require the 'Wago Addons Supporter' \
         Patreon tier ($3/month). Update WOWCTL_WAGO_ACCESS_KEY or run \
         'wowctl config set wago_access_key <key>'."
            .to_string(),
    )
}

impl WagoSource {
    /// Creates a new Wago source with the provided personal access key.
    pub fn new(access_key: String) -> Result<Self> {
        Self::with_base_url(access_key, WAGO_API_BASE.to_string())
    }

    /// Creates a Wago source pointed at a custom API base URL (tests).
    pub fn with_base_url(access_key: String, api_base: String) -> Result<Self> {
        let client = Client::builder()
            .user_agent(format!("wowctl/{}", env!("WOWCTL_VERSION")))
            .timeout(Duration::from_secs(HTTP_TIMEOUT_SECS))
            .build()
            .map_err(|e| WowctlError::Network(format!("Failed to create HTTP client: {e}")))?;

        Ok(Self {
            client,
            access_key,
            api_base,
            circuit_breaker: CircuitBreaker::new(),
        })
    }

    /// Records a request outcome with the circuit breaker. 404s and auth
    /// failures are the caller's problem, not an API outage.
    fn record_circuit_breaker_result<T>(&self, result: &Result<T>) {
        match result {
            Ok(_) => self.circuit_breaker.record_success(),
            Err(WowctlError::AddonNotFound(_))
            | Err(WowctlError::Unauthorized(_))
            | Err(WowctlError::CircuitBreakerOpen) => {}
            Err(_) => self.circuit_breaker.record_failure(),
        }
    }

    /// Executes a request built by `build` with retry/backoff, mapping Wago's
    /// status codes onto wowctl errors. Responses deserialize bare (no `data`
    /// wrapper except where the model itself declares one).
    async fn execute_with_retry<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T> {
        if !self.circuit_breaker.allow_request() {
            return Err(WowctlError::CircuitBreakerOpen);
        }
        let result = self.execute_with_retry_inner(build).await;
        self.record_circuit_breaker_result(&result);
        result
    }

    async fn execute_with_retry_inner<T: DeserializeOwned>(
        &self,
        build: impl Fn() -> reqwest::RequestBuilder,
    ) -> Result<T> {
        let mut attempts = 0;
        let mut backoff_ms = INITIAL_BACKOFF_MS;

        loop {
            attempts += 1;
            match build()
                .bearer_auth(&self.access_key)
                .send()
                .await
            {
                Ok(response) => {
                    let status = response.status();
                    debug!("Wago response status: {}", status);

                    if status.is_success() {
                        return response.json::<T>().await.map_err(|e| {
                            WowctlError::Source(format!("Failed to parse Wago API response: {e}"))
                        });
                    } else if status.as_u16() == 401 {
                        return Err(unauthorized_error());
                    } else if status.as_u16() == 404 {
                        return Err(WowctlError::AddonNotFound(
                            "Addon not found on Wago".to_string(),
                        ));
                    } else if status.as_u16() == 429 {
                        if attempts >= MAX_RETRIES {
                            return Err(WowctlError::Network(
                                "Rate limited by Wago API after multiple retries".to_string(),
                            ));
                        }
                        warn!("Rate limited by Wago API, retrying with backoff...");
                    } else {
                        let error_text = response
                            .text()
                            .await
                            .unwrap_or_else(|_| "Unknown error".to_string());
                        return Err(WowctlError::Source(format!(
                            "Wago API error ({status}): {error_text}"
                        )));
                    }
                }
                Err(e) => {
                    warn!("Network error: {}", e);
                    if attempts >= MAX_RETRIES {
                        return Err(WowctlError::Network(format!(
                            "Failed to connect to Wago API after {MAX_RETRIES} attempts: {e}"
                        )));
                    }
                }
            }

            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
            backoff_ms *= 2;
        }
    }

    /// Fetches addon detail by Wago Addon ID (or slug — the endpoint accepts
    /// both), enforcing is_hidden_from_external.
    async fn get_detail(&self, id_or_slug: &str) -> Result<WagoAddonDetail> {
        let url = format!("{}/addons/{}", self.api_base, id_or_slug);
        let detail: WagoAddonDetail = self
            .execute_with_retry(|| {
                self.client
                    .get(&url)
                    .query(&[("game_version", GAME_VERSION_RETAIL)])
            })
            .await?;
        if detail.is_hidden_from_external {
            return Err(WowctlError::AddonNotFound(format!(
                "Addon '{}' is hidden from external clients by its author",
                detail.slug
            )));
        }
        Ok(detail)
    }

    async fn search_items(&self, query: &str) -> Result<Vec<WagoSearchItem>> {
        let url = format!("{}/addons/_search", self.api_base);
        let response: WagoSearchResponse = self
            .execute_with_retry(|| {
                self.client.get(&url).query(&[
                    ("query", query),
                    ("game_version", GAME_VERSION_RETAIL),
                    ("stability", stability_param(ReleaseChannel::Stable)),
                ])
            })
            .await?;
        Ok(response
            .data
            .into_iter()
            .filter(|item| !item.is_hidden_from_external)
            .collect())
    }
}

fn detail_to_addon_info(detail: &WagoAddonDetail) -> AddonInfo {
    AddonInfo {
        id: detail.id.clone(),
        name: detail.display_name.clone(),
        slug: detail.slug.clone(),
        description: detail.summary.clone(),
        download_count: detail.download_count.map(|d| d as u64),
        source: "wago".to_string(),
    }
}

impl AddonSource for WagoSource {
    async fn search(&self, query: &str, _page: Option<u32>) -> Result<SearchResult> {
        debug!("Searching Wago for: {}", query);
        let items = self.search_items(query).await?;
        let addons: Vec<AddonInfo> = items.iter().map(to_addon_info).collect();
        let count = addons.len() as u32;
        // Wago's search pagination is undocumented; treat results as one page.
        Ok(SearchResult {
            addons,
            page: 1,
            page_size: count,
            total_count: count,
        })
    }

    async fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<VersionInfo> {
        debug!(
            "Getting latest Wago version for {} (channel: {})",
            addon_id, channel
        );
        let detail = self.get_detail(addon_id).await?;
        let release = select_release(&detail.recent_releases, channel).ok_or_else(|| {
            WowctlError::Source(format!(
                "No {channel} release found on Wago for '{}'",
                detail.slug
            ))
        })?;
        to_version_info(release, format!("{}.zip", detail.slug))
    }

    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        // Wago download links are signed and expiring; the Bearer token must
        // accompany the request (ADR-0001).
        crate::sources::download_zip(
            self.client.get(download_url).bearer_auth(&self.access_key),
            download_url,
            destination,
        )
        .await
    }

    async fn resolve_dependencies(
        &self,
        _addon_id: &str,
        _channel: ReleaseChannel,
    ) -> Result<Vec<String>> {
        // The Wago API exposes no dependency data we rely on (issue #8).
        Ok(vec![])
    }

    async fn get_addon_by_slug(&self, slug: &str) -> Result<AddonInfo> {
        debug!("Looking up Wago addon by slug: {}", slug);
        let items = self.search_items(slug).await?;
        if let Some(item) = items.iter().find(|i| item_slug(i).as_deref() == Some(slug)) {
            return Ok(to_addon_info(item));
        }
        // Fallback: the detail endpoint also resolves slugs directly.
        match self.get_detail(slug).await {
            Ok(detail) if detail.slug == slug => Ok(detail_to_addon_info(&detail)),
            Ok(_) | Err(WowctlError::AddonNotFound(_)) => Err(WowctlError::AddonNotFound(
                format!("Addon '{slug}' not found on Wago"),
            )),
            Err(e) => Err(e),
        }
    }

    async fn get_addon_info_by_id(&self, addon_id: &str) -> Result<AddonInfo> {
        let detail = self.get_detail(addon_id).await?;
        Ok(detail_to_addon_info(&detail))
    }

    async fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> Result<HashMap<String, BatchVersionCheck>> {
        if addon_ids.is_empty() {
            return Ok(HashMap::new());
        }
        debug!("Batch checking {} Wago addon(s)", addon_ids.len());
        let url = format!("{}/addons/_recents", self.api_base);
        let body = WagoRecentsRequest {
            game_version: GAME_VERSION_RETAIL,
            addons: addon_ids.iter().map(|s| s.to_string()).collect(),
        };
        let response: WagoRecentsResponse = self
            .execute_with_retry(|| self.client.post(&url).json(&body))
            .await?;

        let mut results = HashMap::new();
        for (id, entry) in &response.addons {
            if let Some(release) = select_release(&entry.recent_releases, channel) {
                results.insert(
                    id.clone(),
                    BatchVersionCheck {
                        addon_id: id.clone(),
                        file_id: None,
                        external_release_id: release.logical_timestamp.map(|t| t.to_string()),
                        version: release.label.clone(),
                        display_name: release.label.clone(),
                        released_at: release.created_at.clone().unwrap_or_default(),
                    },
                );
            }
        }
        debug!(
            "Wago batch check returned {} of {} addon(s)",
            results.len(),
            addon_ids.len()
        );
        Ok(results)
    }
}
```

Adjust imports at the top of the file to match what the code uses (`SearchResult` joins the `crate::addon` import; remove any now-stale `#[allow(dead_code)]` from Task 6 where fields became used).

- [ ] **Step 4: Run tests**

Run: `cargo test --test wago_source && cargo test`
Expected: all PASS.

- [ ] **Step 5: Clippy and commit**

```bash
cargo clippy -- -D warnings
git add src/sources/ tests/wago_source.rs
git commit -m "feat: Wago source client with Bearer auth, recents batch, and hidden-addon filtering"
```

---

### Task 8: `AnySource` enum dispatch and `build_source`

**Files:**
- Modify: `src/sources/mod.rs`
- Test: unit test in `src/sources/mod.rs`

**Interfaces:**
- Consumes: `CurseForgeSource`, `WagoSource`, `Config::{get_api_key, get_wago_access_key}`.
- Produces:
  - `pub enum AnySource { CurseForge(CurseForgeSource), Wago(WagoSource) }` implementing `AddonSource` (ALL methods forwarded explicitly, including the three batch/by-id methods, so CurseForge's batch endpoints keep being used)
  - `impl AnySource { pub fn kind(&self) -> SourceKind }`
  - `pub fn build_source(kind: SourceKind, config: &Config) -> Result<AnySource>` — Wago without a key → `WowctlError::MissingApiKey` with guidance text containing `addons.wago.io/patreon`

- [ ] **Step 1: Write the failing test**

Append to the tests module in `src/sources/mod.rs`:

```rust
    #[test]
    fn build_source_wago_without_key_errors_with_guidance() {
        // Force key absence regardless of the developer machine's env.
        // SAFETY: test-only env mutation; no other test reads this variable
        // concurrently via std::env in this crate's lib tests.
        unsafe { std::env::remove_var("WOWCTL_WAGO_ACCESS_KEY") };
        let config = crate::config::Config {
            wago_access_key: None,
            ..Default::default()
        };
        let err = build_source(SourceKind::Wago, &config).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("addons.wago.io/patreon"), "got: {msg}");
        assert!(msg.contains("WOWCTL_WAGO_ACCESS_KEY"), "got: {msg}");
    }

    #[test]
    fn build_source_wago_with_key_succeeds() {
        let config = crate::config::Config {
            wago_access_key: Some("some-key".to_string()),
            ..Default::default()
        };
        let source = build_source(SourceKind::Wago, &config).unwrap();
        assert_eq!(source.kind(), SourceKind::Wago);
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib sources::tests`
Expected: COMPILE ERROR — `build_source` / `AnySource` not defined.

- [ ] **Step 3: Implement**

Add to `src/sources/mod.rs` (needs `use crate::config::Config;` and `use std::collections::HashMap;` in scope):

```rust
/// A concrete Source behind enum dispatch. The AddonSource trait's async
/// methods (RPIT-in-trait) are not dyn-compatible, so commands hold this
/// enum instead of a trait object.
pub enum AnySource {
    CurseForge(curseforge::CurseForgeSource),
    Wago(wago::WagoSource),
}

impl AnySource {
    pub fn kind(&self) -> SourceKind {
        match self {
            AnySource::CurseForge(_) => SourceKind::CurseForge,
            AnySource::Wago(_) => SourceKind::Wago,
        }
    }
}

impl AddonSource for AnySource {
    async fn search(&self, query: &str, page: Option<u32>) -> Result<SearchResult> {
        match self {
            AnySource::CurseForge(s) => s.search(query, page).await,
            AnySource::Wago(s) => s.search(query, page).await,
        }
    }

    async fn get_latest_version(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<VersionInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_latest_version(addon_id, channel).await,
            AnySource::Wago(s) => s.get_latest_version(addon_id, channel).await,
        }
    }

    async fn download(&self, download_url: &str, destination: &Path) -> Result<PathBuf> {
        match self {
            AnySource::CurseForge(s) => s.download(download_url, destination).await,
            AnySource::Wago(s) => s.download(download_url, destination).await,
        }
    }

    async fn resolve_dependencies(
        &self,
        addon_id: &str,
        channel: ReleaseChannel,
    ) -> Result<Vec<String>> {
        match self {
            AnySource::CurseForge(s) => s.resolve_dependencies(addon_id, channel).await,
            AnySource::Wago(s) => s.resolve_dependencies(addon_id, channel).await,
        }
    }

    async fn get_addon_by_slug(&self, slug: &str) -> Result<AddonInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_by_slug(slug).await,
            AnySource::Wago(s) => s.get_addon_by_slug(slug).await,
        }
    }

    async fn get_addon_info_by_id(&self, addon_id: &str) -> Result<AddonInfo> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_info_by_id(addon_id).await,
            AnySource::Wago(s) => s.get_addon_info_by_id(addon_id).await,
        }
    }

    // Explicit forwarding (not the trait default) so CurseForge's batch
    // endpoints keep being used.
    async fn get_latest_versions_batch(
        &self,
        addon_ids: &[&str],
        channel: ReleaseChannel,
    ) -> Result<HashMap<String, BatchVersionCheck>> {
        match self {
            AnySource::CurseForge(s) => s.get_latest_versions_batch(addon_ids, channel).await,
            AnySource::Wago(s) => s.get_latest_versions_batch(addon_ids, channel).await,
        }
    }

    async fn get_addon_infos_batch(&self, addon_ids: &[String]) -> Result<Vec<AddonInfo>> {
        match self {
            AnySource::CurseForge(s) => s.get_addon_infos_batch(addon_ids).await,
            AnySource::Wago(s) => s.get_addon_infos_batch(addon_ids).await,
        }
    }
}

/// Constructs the client for a Source, resolving its credentials from config.
/// A missing Wago key is a MissingApiKey error — callers that want to treat
/// Wago as "unconfigured" (merged search, update) check
/// `config.get_wago_access_key().is_some()` before calling this.
pub fn build_source(kind: SourceKind, config: &Config) -> Result<AnySource> {
    match kind {
        SourceKind::CurseForge => {
            let api_key = config.get_api_key()?;
            Ok(AnySource::CurseForge(curseforge::CurseForgeSource::new(
                api_key,
            )?))
        }
        SourceKind::Wago => {
            let access_key = config.get_wago_access_key().ok_or_else(|| {
                WowctlError::MissingApiKey(
                    "Wago access key not found. Set WOWCTL_WAGO_ACCESS_KEY or run \
                     'wowctl config set wago_access_key <key>'. Personal access keys \
                     come from https://addons.wago.io/patreon and require the \
                     'Wago Addons Supporter' Patreon tier."
                        .to_string(),
                )
            })?;
            Ok(AnySource::Wago(wago::WagoSource::new(access_key)?))
        }
    }
}
```

- [ ] **Step 4: Run tests, clippy, commit**

```bash
cargo test
cargo clippy -- -D warnings
git add src/sources/mod.rs
git commit -m "feat: AnySource enum dispatch and build_source credential resolution"
```

---

### Task 9: Multi-Source `install`

**Files:**
- Modify: `src/commands/install.rs`
- Modify: `src/utils.rs` (delete `extract_slug_from_url`)
- Modify: `src/commands/remove.rs` (one Source-agnostic regression test)
- Test: unit tests in `src/commands/install.rs`

**Interfaces:**
- Consumes: `parse_addon_spec` (Task 1), `build_source` + `AnySource` (Task 8).
- Produces: `fn check_source_collision(registry: &Registry, slug: &str, kind: SourceKind) -> Result<Option<InstalledAddon>>` (private) — `Ok(Some(existing))` = already installed from the same Source (caller prints and returns), `Err` = installed from a different Source (hard error), `Ok(None)` = not installed.

- [ ] **Step 1: Write the failing tests**

Add a tests module at the bottom of `src/commands/install.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::sources::SourceKind;

    fn registry_with(slug: &str, source: &str) -> Registry {
        let mut registry = Registry::default();
        registry.add(InstalledAddon {
            name: slug.to_string(),
            slug: slug.to_string(),
            version: "1.0.0".to_string(),
            source: source.to_string(),
            addon_id: "1".to_string(),
            directories: vec![],
            is_dependency: false,
            required_by: vec![],
            installed_file_id: None,
            display_name: None,
            channel: None,
            ignored: None,
            game_versions: None,
            released_at: None,
            auto_update: None,
            external_release_id: None,
        });
        registry
    }

    #[test]
    fn not_installed_returns_none() {
        let registry = Registry::default();
        let result = check_source_collision(&registry, "classcodex", SourceKind::Wago).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn same_source_returns_existing() {
        let registry = registry_with("classcodex", "wago");
        let existing = check_source_collision(&registry, "classcodex", SourceKind::Wago)
            .unwrap()
            .unwrap();
        assert_eq!(existing.slug, "classcodex");
    }

    #[test]
    fn different_source_is_hard_error() {
        let registry = registry_with("classcodex", "curseforge");
        let err = check_source_collision(&registry, "classcodex", SourceKind::Wago).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("curseforge"), "got: {msg}");
        assert!(msg.contains("wowctl remove classcodex"), "got: {msg}");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib commands::install`
Expected: COMPILE ERROR — `check_source_collision` not defined.

- [ ] **Step 3: Implement collision check and rewire install**

In `src/commands/install.rs`:

1. Replace the imports of `CurseForgeSource` and `extract_slug_from_url`:

```rust
use crate::sources::{AddonSource, build_source, parse_addon_spec, SourceKind};
```

(drop `use crate::sources::curseforge::CurseForgeSource;`; remove `extract_slug_from_url` from the `crate::utils` import list).

2. Add the helper:

```rust
/// Guards the Registry's one-Slug-one-Source invariant on install.
/// Ok(Some(existing)) = already installed from this same Source;
/// Err = installed from a different Source (user must remove first);
/// Ok(None) = not installed.
fn check_source_collision(
    registry: &Registry,
    slug: &str,
    kind: SourceKind,
) -> Result<Option<InstalledAddon>> {
    match registry.get(slug) {
        None => Ok(None),
        Some(existing) if existing.source == kind.as_str() => Ok(Some(existing.clone())),
        Some(existing) => Err(WowctlError::Source(format!(
            "'{slug}' is already installed from {}. An addon can only have one \
             update origin — run 'wowctl remove {slug}' first, then install it from {}.",
            existing.source,
            kind.as_str()
        ))),
    }
}
```

3. Rewire the top of `install`:

```rust
pub async fn install(addon: &str, channel_override: Option<ReleaseChannel>) -> Result<()> {
    let config = Config::load()?;
    let addon_dir = config.get_addon_dir()?;
    let channel = config.resolve_channel(channel_override);

    let (source_kind, slug) = parse_addon_spec(addon)?;

    let mut registry = Registry::load()?;

    if let Some(existing) = check_source_collision(&registry, &slug, source_kind)? {
        println!(
            "{} is already installed (version {})",
            existing.name.color_cyan(),
            existing.version.color_green()
        );
        return Ok(());
    }

    // Errors here include the missing-Wago-key guidance from build_source.
    let source = Arc::new(build_source(source_kind, &config)?);

    let addon_info = source.get_addon_by_slug(&slug).await?;
    // ... rest of the function unchanged ...
```

(The old `let api_key = config.get_api_key()?;`, the `starts_with("http")` block, the `CurseForgeSource::new` call, and the old already-installed check are all removed. The dependency BFS, the download/extract phase, and the registry write remain byte-for-byte the same — they already go through the `AddonSource` trait, and `AnySource: Send + Sync` holds for the `tokio::spawn` closures.)

4. Delete `extract_slug_from_url` from `src/utils.rs` (no other callers; `cargo check` confirms).

- [ ] **Step 4: Add the Source-agnostic remove regression test**

In `src/commands/remove.rs` tests, `make_addon` builds a `"curseforge"`-sourced addon. Add one test proving `remove` logic never consults the source (user story 17):

```rust
    #[test]
    fn remove_works_for_wago_sourced_addon() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("ClassCodex");
        std::fs::create_dir_all(&dir).unwrap();

        let mut registry = Registry::default();
        let mut addon = make_addon("classcodex", vec!["ClassCodex"]);
        addon.source = "wago".to_string();
        registry.add(addon);

        let removed = registry.remove("classcodex").unwrap();
        assert_eq!(removed.source, "wago");
        assert!(registry.get("classcodex").is_none());
    }
```

- [ ] **Step 5: Run tests, clippy, commit**

```bash
cargo test
cargo clippy -- -D warnings
git add src/commands/install.rs src/commands/remove.rs src/utils.rs
git commit -m "feat: install dispatches by parsed source; cross-source slug collision is a hard error"
```

---

### Task 10: Multi-Source `update`

**Files:**
- Modify: `src/commands/update.rs`
- Test: unit tests in `src/commands/update.rs`

**Interfaces:**
- Consumes: `build_source`, `AnySource`, `SourceKind`, `BatchVersionCheck`, `is_update_available` (Task 3).
- Produces (private to the module):
  - `fn group_by_source(addons: &[InstalledAddon]) -> (Vec<(SourceKind, Vec<InstalledAddon>)>, Vec<InstalledAddon>)` — groups in deterministic order (CurseForge first, then Wago); second element = addons whose `source` string is unknown (skipped with a warning).
  - `UpdateInfo` gains `kind: SourceKind`.
  - `needs_metadata_refresh` becomes Source-aware.
  - `check_updates_sequential` and `refresh_stale_metadata` take `&AnySource` instead of `&CurseForgeSource`.

- [ ] **Step 1: Write the failing tests**

Append to the `update.rs` tests module (extend `make_installed` usage with a source-parameterized helper):

```rust
    fn make_sourced(slug: &str, source: &str) -> InstalledAddon {
        let mut a = make_installed(None, None, "1.0");
        a.slug = slug.to_string();
        a.name = slug.to_string();
        a.source = source.to_string();
        a
    }

    #[test]
    fn group_by_source_splits_and_orders() {
        let addons = vec![
            make_sourced("w1", "wago"),
            make_sourced("c1", "curseforge"),
            make_sourced("w2", "wago"),
        ];
        let (groups, unknown) = group_by_source(&addons);
        assert!(unknown.is_empty());
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].0, SourceKind::CurseForge);
        assert_eq!(groups[0].1.len(), 1);
        assert_eq!(groups[1].0, SourceKind::Wago);
        assert_eq!(groups[1].1.len(), 2);
    }

    #[test]
    fn group_by_source_flags_unknown_sources() {
        let addons = vec![
            make_sourced("c1", "curseforge"),
            make_sourced("x1", "wowinterface"),
        ];
        let (groups, unknown) = group_by_source(&addons);
        assert_eq!(groups.len(), 1);
        assert_eq!(unknown.len(), 1);
        assert_eq!(unknown[0].slug, "x1");
    }

    #[test]
    fn group_by_source_empty_input() {
        let (groups, unknown) = group_by_source(&[]);
        assert!(groups.is_empty());
        assert!(unknown.is_empty());
    }

    #[test]
    fn wago_needs_refresh_when_release_id_missing() {
        let mut a = make_sourced("w1", "wago");
        a.external_release_id = None;
        a.installed_file_id = None;
        assert!(needs_metadata_refresh(&a));
        a.external_release_id = Some("123".to_string());
        assert!(!needs_metadata_refresh(&a));
    }

    #[test]
    fn curseforge_needs_refresh_when_file_id_missing() {
        let mut a = make_sourced("c1", "curseforge");
        a.installed_file_id = None;
        assert!(needs_metadata_refresh(&a));
        a.installed_file_id = Some(42);
        assert!(!needs_metadata_refresh(&a));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib commands::update`
Expected: COMPILE ERROR — `group_by_source` not defined; `needs_metadata_refresh` signature mismatch behavior.

- [ ] **Step 3: Implement grouping and Source-aware staleness**

In `src/commands/update.rs`, replace the `CurseForgeSource` import with:

```rust
use crate::sources::{AddonSource, AnySource, BatchVersionCheck, SourceKind, build_source};
use crate::sources::curseforge::CurseForgeSource;
```

Add:

```rust
/// Groups addons by their registry Source in deterministic order
/// (CurseForge, then Wago). Addons with an unrecognized source string are
/// returned separately so the caller can warn and skip them.
fn group_by_source(
    addons: &[InstalledAddon],
) -> (Vec<(SourceKind, Vec<InstalledAddon>)>, Vec<InstalledAddon>) {
    let mut unknown = Vec::new();
    let mut cf = Vec::new();
    let mut wago = Vec::new();
    for addon in addons {
        match addon.source.parse::<SourceKind>() {
            Ok(SourceKind::CurseForge) => cf.push(addon.clone()),
            Ok(SourceKind::Wago) => wago.push(addon.clone()),
            Err(_) => unknown.push(addon.clone()),
        }
    }
    let mut groups = Vec::new();
    if !cf.is_empty() {
        groups.push((SourceKind::CurseForge, cf));
    }
    if !wago.is_empty() {
        groups.push((SourceKind::Wago, wago));
    }
    (groups, unknown)
}
```

Replace `needs_metadata_refresh`:

```rust
/// A registry entry is stale when it lacks the release identity its Source
/// uses for update detection.
fn needs_metadata_refresh(addon: &InstalledAddon) -> bool {
    match addon.source.parse::<SourceKind>() {
        Ok(SourceKind::Wago) => addon.external_release_id.is_none(),
        _ => addon.installed_file_id.is_none(),
    }
}
```

- [ ] **Step 4: Restructure the update flow**

Replace the single-source section of `update()` (from `let source = Arc::new(CurseForgeSource::new(api_key)?);` through the batch/sequential check and the `fix_version_strings`/`refresh_stale_metadata` calls) with the per-Source loop. `UpdateInfo` gains `kind: SourceKind`; keep the rest of the struct unchanged:

```rust
struct UpdateInfo {
    slug: String,
    name: String,
    current_version: String,
    new_version: String,
    addon_id: String,
    channel: ReleaseChannel,
    kind: SourceKind,
}
```

New flow inside `update()` (replacing the old `let api_key = ...` and `let source = ...` lines and the whole "Checking for updates" section):

```rust
    let (groups, unknown) = group_by_source(&addons_to_check);
    for addon in &unknown {
        println!(
            "  {} Skipping {}: unknown source '{}'",
            "Warning:".color_yellow(),
            addon.slug.color_cyan(),
            addon.source
        );
    }

    println!("Checking for updates...");
    let mut updates = Vec::new();
    let mut sources: HashMap<SourceKind, Arc<AnySource>> = HashMap::new();
    let mut fixed = 0;
    let mut stale_count = 0;

    for (kind, group) in &groups {
        // One Source's missing credentials or outage must not block the other
        // (user stories 8 and 20).
        let source = match build_source(*kind, &config) {
            Ok(s) => Arc::new(s),
            Err(e) => {
                println!(
                    "  {} Skipping {} {} addon(s): {}",
                    "Warning:".color_yellow(),
                    group.len(),
                    kind,
                    e
                );
                continue;
            }
        };
        sources.insert(*kind, Arc::clone(&source));

        let addon_ids: Vec<&str> = group.iter().map(|a| a.addon_id.as_str()).collect();
        match source
            .get_latest_versions_batch(&addon_ids, default_channel)
            .await
        {
            Ok(batch_map) => {
                let mut missed_addons = Vec::new();
                for installed in group {
                    let addon_channel =
                        resolve_addon_channel(installed, channel_override, default_channel);
                    if let Some(check) = batch_map.get(&installed.addon_id) {
                        if is_update_available(installed, check) {
                            updates.push(UpdateInfo {
                                slug: installed.slug.clone(),
                                name: installed.name.clone(),
                                current_version: installed.version.clone(),
                                new_version: check.version.clone(),
                                addon_id: installed.addon_id.clone(),
                                channel: addon_channel,
                                kind: *kind,
                            });
                        }
                    } else {
                        debug!(
                            "Addon {} not in batch result, will check individually",
                            installed.slug
                        );
                        missed_addons.push(installed.clone());
                    }
                }
                if !missed_addons.is_empty() {
                    check_updates_sequential(
                        &source,
                        *kind,
                        &missed_addons,
                        channel_override,
                        default_channel,
                        &mut updates,
                    )
                    .await;
                }
            }
            Err(e) => {
                warn!(
                    "Batch update check for {} failed ({}), falling back to sequential checks",
                    kind, e
                );
                check_updates_sequential(
                    &source,
                    *kind,
                    group,
                    channel_override,
                    default_channel,
                    &mut updates,
                )
                .await;
            }
        }

        // CurseForge-only repair: version strings extracted with an older heuristic.
        if let AnySource::CurseForge(cf) = source.as_ref() {
            fixed += fix_version_strings(cf, &mut registry, group);
        }

        stale_count += refresh_stale_metadata(
            &source,
            &mut registry,
            group,
            channel_override,
            default_channel,
        )
        .await;
    }

    if fixed + stale_count > 0 {
        registry.save()?;
    }
```

Signature/body updates to the helpers:

```rust
async fn check_updates_sequential(
    source: &AnySource,
    kind: SourceKind,
    addons: &[InstalledAddon],
    channel_override: Option<ReleaseChannel>,
    default_channel: ReleaseChannel,
    updates: &mut Vec<UpdateInfo>,
) {
    // body identical to today's, except the Ok arm builds a
    // BatchVersionCheck::from_version_info and uses is_update_available
    // (already done in Task 3), and UpdateInfo now gets `kind,`.
}

async fn refresh_stale_metadata(
    source: &AnySource,
    registry: &mut Registry,
    addons: &[InstalledAddon],
    channel_override: Option<ReleaseChannel>,
    default_channel: ReleaseChannel,
) -> usize {
    // body identical; also copy version_info.external_release_id into the
    // entry alongside installed_file_id (done in Task 3 step 4.6).
}
```

`fix_version_strings` keeps its `&CurseForgeSource` first parameter (unchanged).

In the download phase, resolve each update's source from the map (replace `let source = Arc::clone(&source);` at the top of the `for update in updates` loop):

```rust
    for update in updates {
        let source = Arc::clone(
            sources
                .get(&update.kind)
                .expect("update only queued for sources that were built"),
        );
        // ... rest of the download task unchanged ...
```

Add `use std::collections::HashMap;` to the imports.

- [ ] **Step 5: Run tests, clippy, commit**

```bash
cargo test
cargo clippy -- -D warnings
git add src/commands/update.rs
git commit -m "feat: update dispatches per source with graceful per-source and per-addon degradation"
```

---

### Task 11: Merged multi-Source `search` with `--source` filter

**Files:**
- Modify: `src/commands/search.rs`
- Modify: `src/main.rs` (flag + call site)

**Interfaces:**
- Consumes: `build_source`, `AnySource`, `SourceKind` (`clap::ValueEnum`).
- Produces: `pub async fn search(query: &str, page: Option<u32>, source_filter: Option<SourceKind>) -> Result<()>` — new third parameter.

Behavior (user stories 6–8): no `--source` → query every configured Source, print results **grouped by Source** with a header per group, no cross-Source dedup; a Source that fails prints a warning and the others still print; unconfigured Wago prints a one-line dimmed note. `--source wago` without a key → hard error (the `build_source` MissingApiKey guidance). CurseForge output (including pagination footer) stays byte-compatible for the no-flag, Wago-unconfigured case except for the added header and note.

- [ ] **Step 1: Rewrite `src/commands/search.rs`**

```rust
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
```

- [ ] **Step 2: Wire up `src/main.rs`**

Add to the `Search` variant:

```rust
    #[command(about = "Search for addons")]
    Search {
        #[arg(help = "Search query")]
        query: String,

        #[arg(long, help = "Page number (default: 1)")]
        page: Option<u32>,

        #[arg(long, value_enum, help = "Limit search to one source: curseforge or wago")]
        source: Option<wowctl::sources::SourceKind>,
    },
```

And the dispatch arm:

```rust
        Commands::Search { query, page, source } => {
            wowctl::commands::search::search(&query, page, source).await
        }
```

- [ ] **Step 3: Build and verify by hand**

```bash
cargo build
target/debug/wowctl search --help          # shows --source with curseforge|wago values
target/debug/wowctl search deadly-boss-mods  # CurseForge section + dimmed Wago-skipped note (no wago key)
target/debug/wowctl search foo --source wago # hard error mentioning addons.wago.io/patreon (no wago key)
```

Expected outputs as annotated. (These hit the real CurseForge API via the embedded key; the wago paths need no key to demonstrate.)

- [ ] **Step 4: Run tests, clippy, commit**

```bash
cargo test
cargo clippy -- -D warnings
git add src/commands/search.rs src/main.rs
git commit -m "feat: merged multi-source search with --source filter and graceful wago skip"
```

---

### Task 12: `info` URL, `config init/show/set`, example config

**Files:**
- Modify: `src/commands/info.rs`
- Modify: `src/commands/config.rs`
- Modify: `config.toml.example`
- Test: unit test in `src/commands/info.rs`

**Interfaces:**
- Consumes: `Config.wago_access_key`, `get_wago_access_key`, `WagoSource`, `WowctlError::Unauthorized`.
- Produces: `fn addon_page_url(source: &str, slug: &str) -> Option<String>` (private to `commands::info`).

- [ ] **Step 1: Write the failing test (info URL)**

Append to the tests module in `src/commands/info.rs`:

```rust
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
```

Run: `cargo test --lib commands::info` — COMPILE ERROR (function missing).

- [ ] **Step 2: Implement in `src/commands/info.rs`**

```rust
/// The web page for an addon on its Source, if the Source is known.
fn addon_page_url(source: &str, slug: &str) -> Option<String> {
    match source {
        "curseforge" => Some(format!("https://www.curseforge.com/wow/addons/{slug}")),
        "wago" => Some(format!("https://addons.wago.io/addons/{slug}")),
        _ => None,
    }
}
```

Replace the trailing `if installed_addon.source == "curseforge" { ... }` block in `info()` with:

```rust
    if let Some(url) = addon_page_url(&installed_addon.source, &installed_addon.slug) {
        println!("  {}: {}", "URL".color_bold(), url.color_blue());
    }
```

Run: `cargo test --lib commands::info` — PASS. (`list` already prints `addon.source` per row and `info` already prints the Source field, so user story 13 needs no further change.)

- [ ] **Step 3: `config set` accepts `wago_access_key`**

In `src/commands/config.rs` `set()`, add an arm after `"curseforge_api_key"`:

```rust
        "wago_access_key" => {
            config.wago_access_key = Some(value.to_string());
            println!("Set Wago access key");
        }
```

And extend the unknown-key error text: `Valid keys: addon_dir, curseforge_api_key, wago_access_key, color, default_release_channel`.

- [ ] **Step 4: `config show` indicates the Wago key without printing it**

In `show()`, after the CurseForge key block (story 11 says indicate, don't print — no masking either):

```rust
    let wago_env = std::env::var("WOWCTL_WAGO_ACCESS_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty());
    let wago_status = if wago_env.is_some() {
        "Configured (from environment)".color_green().to_string()
    } else if config.wago_access_key.is_some() {
        "Configured".color_green().to_string()
    } else {
        "Not set (Wago source disabled)".color_yellow().to_string()
    };
    println!("  {}: {}", "Wago access key".color_bold(), wago_status);
```

(Note: `color_green()` returns an `impl Display` wrapper in this codebase's `ColorExt`; if `.to_string()` on it doesn't compile, restructure as three `println!` branches like the CurseForge block above it.)

- [ ] **Step 5: `config init` optionally prompts for the Wago key**

In `init()`, insert between the CurseForge step and the addon-directory step (renumber the directory step's header to "Step 3"):

```rust
    println!("{}", "Step 2: Wago Addons access key (optional)".color_bold());
    println!("Needed only for Wago-exclusive addons (installed with 'wago:<slug>').");
    println!("Keys come from https://addons.wago.io/patreon and require the");
    println!("'Wago Addons Supporter' Patreon tier. Press Enter to skip.");
    println!();

    let wago_key: String = Input::new()
        .with_prompt("Enter your Wago access key (optional)")
        .allow_empty(true)
        .interact_text()
        .map_err(|e| WowctlError::Config(format!("Failed to read input: {e}")))?;

    let wago_key = wago_key.trim().to_string();
    if !wago_key.is_empty() {
        println!("{}", "Verifying Wago access key...".color_dimmed());
        let test_wago = crate::sources::wago::WagoSource::new(wago_key.clone())?;
        match test_wago.search("weakauras", None).await {
            Ok(_) => println!("{}", "Wago access key verified successfully!".color_green()),
            Err(WowctlError::Unauthorized(msg)) => {
                return Err(WowctlError::Config(format!(
                    "Wago access key verification failed: {msg}"
                )));
            }
            Err(e) => {
                // Network hiccup or API drift: keep the key, warn the user.
                println!(
                    "{}",
                    format!("Warning: could not verify Wago key ({e}); saving it anyway.")
                        .color_yellow()
                );
            }
        }
        config.wago_access_key = Some(wago_key);
    }
    println!();
```

- [ ] **Step 6: Update `config.toml.example`**

Add alongside the CurseForge key entry:

```toml
# Wago Addons personal access key (optional). Needed only for Wago-exclusive
# addons ('wowctl install wago:<slug>'). Keys come from
# https://addons.wago.io/patreon and require the 'Wago Addons Supporter'
# Patreon tier (~$3/month). Can also be set via WOWCTL_WAGO_ACCESS_KEY.
# wago_access_key = "your-wago-access-key"
```

- [ ] **Step 7: Run tests, clippy, verify by hand, commit**

```bash
cargo test
cargo clippy -- -D warnings
cargo build
target/debug/wowctl config show   # shows "Wago access key: Not set (Wago source disabled)"
target/debug/wowctl config set wago_access_key dummy && target/debug/wowctl config show  # "Configured"
target/debug/wowctl config set wago_access_key ""  # optional cleanup of the dummy on your machine
git add src/commands/info.rs src/commands/config.rs config.toml.example
git commit -m "feat: wago key in config init/show/set; wago page URL in info"
```

(Note: `config set wago_access_key ""` stores an empty string, which `get_wago_access_key` treats as unset — acceptable cleanup; or edit config.toml manually.)

---

### Task 13: Documentation

**Files:**
- Modify: `README.md`
- Modify: `AGENTS.md`

- [ ] **Step 1: README section**

Add a "Wago Addons support" section to `README.md` (place near the existing API-key/setup docs), covering exactly these points:

```markdown
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
```

- [ ] **Step 2: AGENTS.md updates**

1. In **Project Structure**, add `wago.rs # Wago Addons API client` under `sources/` and update the `mod.rs` line to `# AddonSource trait, SourceKind, AnySource dispatch, spec parsing`.
2. Add a new section after "CurseForge API Details":

```markdown
## Wago API Details (unofficial — see ADR-0001)

- **Base URL:** `https://addons.wago.io/api/external`
- **Endpoints used:** `GET /addons/_search?query=&game_version=retail&stability=`,
  `GET /addons/{id}?game_version=retail` (accepts ID or slug),
  `POST /addons/_recents` with `{"game_version":"retail","addons":[ids]}`.
- **Auth:** `Authorization: Bearer <personal access key>` on every call AND every
  download (links are signed/expiring). Key source: addons.wago.io/patreon
  ("Wago Addons Supporter" tier). Never embed a Wago key in builds.
- **Release identity:** `logical_timestamp` (stored as `external_release_id` in
  the registry) — Wago has no numeric file IDs.
- **Stability tiers** stable/beta/alpha map 1:1 to wowctl release channels.
- **Respect `is_hidden_from_external`:** filtered from search, install, update.
- **Reference implementation:** WowUp's
  `wowup-electron/src/app/addon-providers/wago-addon-provider.ts` — re-read it
  when the API drifts.
- **Motivating/acceptance addon:** ClassCodex (slug `classcodex`, ID `rNkynwKa`).
```

3. In **API Key**, mention: "Wago access key (optional): `WOWCTL_WAGO_ACCESS_KEY` or `wago_access_key` in config.toml; see Wago API Details."
4. In **Key Dependencies**, add `wiremock` (dev) — HTTP-boundary tests.

- [ ] **Step 3: Commit**

```bash
git add README.md AGENTS.md
git commit -m "docs: wago source setup, API contract notes, and supporter-tier requirement"
```

---

### Task 14: Live acceptance against the real Wago API

The wiremock fixtures pin an **assumed** contract (from WowUp's source). This task validates it with a real key against ClassCodex and fixes any drift. Requires `WOWCTL_WAGO_ACCESS_KEY` in the environment (the repo owner has a verified personal key; if you are an agent without one, complete Steps 1–2 and hand Steps 3–5 to the user as a checklist).

**Files:**
- Create: `tests/wago_live.rs`

- [ ] **Step 1: Add the ignored live test**

Create `tests/wago_live.rs`:

```rust
//! Live acceptance test against the real Wago API. Ignored by default.
//! Run with a real key:
//!   WOWCTL_WAGO_ACCESS_KEY=<key> cargo test --test wago_live -- --ignored --nocapture

use wowctl::addon::ReleaseChannel;
use wowctl::sources::AddonSource;
use wowctl::sources::wago::WagoSource;

#[tokio::test]
#[ignore = "hits the live Wago API; requires WOWCTL_WAGO_ACCESS_KEY"]
async fn classcodex_search_resolve_and_download() {
    let key = std::env::var("WOWCTL_WAGO_ACCESS_KEY")
        .expect("set WOWCTL_WAGO_ACCESS_KEY to run the live test");
    let source = WagoSource::new(key).unwrap();

    // Slug resolution (user story 1); issue #8 records the expected Wago ID.
    let info = source.get_addon_by_slug("classcodex").await.unwrap();
    assert_eq!(info.id, "rNkynwKa");
    assert_eq!(info.source, "wago");

    // Latest stable release resolves with a download link and release identity.
    let v = source
        .get_latest_version(&info.id, ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(!v.version.is_empty());
    assert!(v.external_release_id.is_some());

    // Signed download with Bearer auth yields a real zip (user story 18).
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("classcodex.zip");
    source.download(&v.download_url, &dest).await.unwrap();
    let bytes = std::fs::read(&dest).unwrap();
    assert_eq!(&bytes[..4], b"PK\x03\x04");

    // Merged-search visibility (user story 6).
    let results = source.search("classcodex", None).await.unwrap();
    assert!(results.addons.iter().any(|a| a.slug == "classcodex"));

    // Batch recents contract (user story 5).
    let checks = source
        .get_latest_versions_batch(&[info.id.as_str()], ReleaseChannel::Stable)
        .await
        .unwrap();
    assert!(checks.contains_key(&info.id));
}
```

- [ ] **Step 2: Verify it compiles and stays ignored in CI**

Run: `cargo test --test wago_live`
Expected: `1 ignored`, exit 0.

- [ ] **Step 3: Run the live test with a real key**

Run: `WOWCTL_WAGO_ACCESS_KEY=<real key> cargo test --test wago_live -- --ignored --nocapture`

If any assertion or deserialization fails, the live contract differs from the WowUp-derived assumption: adjust the serde field names/shapes in `src/sources/wago.rs`, mirror the change in the `tests/wago_source.rs` fixtures and `src/sources/wago.rs` unit-test JSON, and note the corrected contract in AGENTS.md's Wago section. Re-run until green.

- [ ] **Step 4: Manual CLI acceptance (real key, throwaway addon dir)**

```bash
cargo build --release
export WOWCTL_WAGO_ACCESS_KEY=<real key>
mkdir -p /tmp/wowctl-accept
target/release/wowctl --addon-dir /tmp/wowctl-accept install wago:classcodex   # installs
target/release/wowctl --addon-dir /tmp/wowctl-accept list                      # shows classcodex ... wago
target/release/wowctl --addon-dir /tmp/wowctl-accept info classcodex           # Source: wago, URL: addons.wago.io/...
target/release/wowctl --addon-dir /tmp/wowctl-accept update                    # "All addons are up to date."
target/release/wowctl --addon-dir /tmp/wowctl-accept install classcodex        # ERROR: already installed from wago (collision, story 15 — bare slug means CurseForge)
target/release/wowctl --addon-dir /tmp/wowctl-accept search classcodex         # grouped CurseForge + Wago sections
target/release/wowctl --addon-dir /tmp/wowctl-accept remove classcodex         # removes cleanly
```

Note: `install wago:classcodex` writes to the real registry (`registry.toml` in the platform data dir) even with `--addon-dir` overridden — remove the addon at the end (last step) to leave the registry clean.

- [ ] **Step 5: Commit**

```bash
git add tests/wago_live.rs
# plus any contract fixes from Step 3 (src/sources/wago.rs, tests/wago_source.rs, AGENTS.md)
git commit -m "test: live Wago acceptance test (ignored by default) for ClassCodex"
```

---

## Self-Review Notes (performed while writing)

**Spec coverage — user story → task:** 1→T9, 2→T1, 3→T1 (bare/CF-URL tests), 4→T1, 5→T10, 6→T11, 7→T11, 8→T8+T11, 9→T2, 10→T12, 11→T12, 12→T6+T7, 13→already satisfied (`list` prints `addon.source`; `info` prints Source) + verified in T14, 14→T12, 15→T9, 16→T6, 17→T9 (remove regression test; `ignore`/`unignore` are registry-only by construction), 18→T4 shared `download_zip` used by both sources, 19→T6+T7, 20→T10, 21→T5+T8. Implementation decisions: enum dispatch (T8), registry identity unchanged / no migration (T3 uses `serde(default)`; verified by backward-compat tests), Wago deps empty (T7), retail pinned (T7 constants), merged output grouped without dedup (T11), adopt untouched.

**Type consistency spot-checks:** `BatchVersionCheck` fields match across T3 (definition), T7 (Wago construction), T5 (move); `check_updates_sequential` gains `kind` parameter in T10 and its T3 body change already builds `UpdateInfo` — T10 adds `kind: *kind`-style field at both call sites; `SourceKind::as_str` values match registry literals `"curseforge"`/`"wago"` everywhere; `with_base_url` signatures match between T4 (CF) and T7 (Wago) and their tests.

**Known judgment calls (implementer need not re-decide):** Wago search pins `stability=stable` (WowUp's default) — revisit only if T14 shows beta-only addons missing from search; Wago search is unpaginated (single page); `file_size: 0` makes the disk-space check a no-op for Wago (API reports no sizes); `config show` deliberately does NOT mask-print the Wago key (story 11 says indicate only).

