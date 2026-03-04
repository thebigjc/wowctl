//! wowctl - World of Warcraft Addon Manager
//!
//! A command-line tool for managing World of Warcraft Retail addons.
//! Supports searching, installing, updating, and removing addons from CurseForge
//! with automatic dependency resolution.

pub mod addon;
pub(crate) mod circuit_breaker;
pub mod colors;
pub mod commands;
pub mod config;
pub mod error;
pub mod registry;
pub mod sources;
pub mod utils;
