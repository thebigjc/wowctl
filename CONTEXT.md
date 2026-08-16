# wowctl

A CLI addon manager for World of Warcraft: it searches addon platforms, installs addon zips into the WoW addon directory, and keeps them updated.

## Language

**Source**:
An addon distribution platform wowctl can search, download from, and check for updates (e.g. CurseForge, Wago Addons). Every installed addon records which Source it came from.
_Avoid_: provider, platform, repository

**Addon**:
A WoW modification distributed as a zip whose top-level folders are installed into the addon directory. One Addon may own several directories.
_Avoid_: mod, package

**Slug**:
The human-readable identifier for an Addon (e.g. `classcodex`). The registry is keyed by Slug.
_Avoid_: name (that's the display name), id (that's the Source-assigned identifier)

**Addon ID**:
The Source-assigned identifier for an Addon (e.g. a CurseForge numeric mod id). Only meaningful within its Source.

**Registry**:
wowctl's local record of every managed Addon: its Slug, Source, version, directories, and dependency links.

**Managed / Unmanaged**:
A Managed addon is tracked in the Registry; an Unmanaged addon exists in the addon directory but is unknown to wowctl until adopted.
_Avoid_: tracked/untracked

**Adopt**:
Identify an Unmanaged addon on disk and bring it into the Registry under its Source.

**Release Channel**:
The stability tier of a release a user is willing to install: release, beta, or alpha.

**Flavor**:
A WoW client variant (Retail, Classic). wowctl currently targets Retail only.
_Avoid_: game version (that's the patch number, e.g. 11.0.2)
