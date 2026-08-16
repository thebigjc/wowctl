# Wago source uses the undocumented external API with a personal access key

Wago Addons has no documented consumer API — docs.wago.io covers publishing only (`POST /api/projects/<id>/version`). The consumer surface (`https://addons.wago.io/api/external/*`: `_search`, `/addons/{id}`, `_recents`, `_match`, signed download links) is known only from WowUp's source, exists under a 2021 Wago–WowUp partnership, and has no published third-party terms. We use it anyway, authenticated with a personal access key, because it is the only way to manage Wago-exclusive addons (the motivating case: ClassCodex, delisted from CurseForge in mid-2026).

## Consequences

- **Auth is Bearer-token on every call, downloads included.** The two token sources are an ad-display flow (impossible in a CLI) and the personal access key from `addons.wago.io/patreon`, whose availability is the "ad free usage of … addon managers" benefit of the $3/month "Wago Addons Supporter" Patreon tier. wowctl's Wago source therefore effectively requires that subscription.
- **No embedded Wago key in release builds** — unlike the embedded CurseForge fallback key. Keys are personal; we have no distribution agreement with Wago. Precedence: `WOWCTL_WAGO_ACCESS_KEY` env var, then `wago_access_key` in config.toml.
- **Respect `is_hidden_from_external`**: addons whose authors opted out of third-party clients must not be surfaced or downloaded.
- The API is unversioned and undocumented; it can change or be revoked without notice. Wago support is documented as unofficial, and breakage here is expected to be fixed by re-reading WowUp's provider (`wowup-electron/src/app/addon-providers/wago-addon-provider.ts`), our reference implementation.

## Considered options

- **Ad-token flow (WowUp's default)** — rejected: requires embedding a browser view to display Wago's ad; meaningless in a terminal.
- **Documented publishing API only** — rejected: it cannot search, resolve, or download addons.
- **Generic "install from URL" without a real source** — rejected: leaves addons invisible to `wowctl update`, which defeats the tool's purpose.
