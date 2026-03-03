//! Error types for wowctl.

use thiserror::Error;

/// Main error type for wowctl operations.
#[derive(Error, Debug)]
pub enum WowctlError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("Registry error: {0}")]
    Registry(String),

    #[error("Addon source error: {0}")]
    Source(String),

    #[error("CurseForge API error: {0}")]
    CurseForge(String),

    #[error("Addon not found: {0}")]
    AddonNotFound(String),

    #[error("Distribution denied: {0}")]
    DistributionDenied(String),

    #[error("Network error: {0}")]
    Network(String),

    #[error(
        "CurseForge API is temporarily unavailable after multiple consecutive failures. Please try again in a few seconds."
    )]
    CircuitBreakerOpen,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Serialization error: {0}")]
    Serialization(String),

    #[error("Extraction error: {0}")]
    Extraction(String),

    #[error("Invalid addon directory: {0}")]
    InvalidAddonDir(String),

    #[error("Missing API key: {0}")]
    MissingApiKey(String),

    #[error("Dependency error: {0}")]
    Dependency(String),
}

pub type Result<T> = std::result::Result<T, WowctlError>;

impl From<reqwest::Error> for WowctlError {
    fn from(err: reqwest::Error) -> Self {
        WowctlError::Network(err.to_string())
    }
}

impl From<serde_json::Error> for WowctlError {
    fn from(err: serde_json::Error) -> Self {
        WowctlError::Serialization(err.to_string())
    }
}

impl From<toml::de::Error> for WowctlError {
    fn from(err: toml::de::Error) -> Self {
        WowctlError::Serialization(err.to_string())
    }
}

impl From<toml::ser::Error> for WowctlError {
    fn from(err: toml::ser::Error) -> Self {
        WowctlError::Serialization(err.to_string())
    }
}

impl From<zip::result::ZipError> for WowctlError {
    fn from(err: zip::result::ZipError) -> Self {
        WowctlError::Extraction(err.to_string())
    }
}
