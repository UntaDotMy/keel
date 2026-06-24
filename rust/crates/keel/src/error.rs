//! Purpose: Crate-wide typed error enum replacing stringly-typed `Result<T, String>`.
//! Caller: Any module that currently returns `Result<T, String>` — migrate one file at a time.
//! Dependencies: thiserror for derive macros, std::error::Error for Display + source chain.
//! Main Functions: `KeelError` enum with domain-specific variants and blanket `From<String>`.
//! Side Effects: None — pure type definition.

use thiserror::Error;

/// Central error type for the `keel` crate.
///
/// Variant selection: use the most specific variant that fits. The `Io`, `Json`,
/// and `Sqlite` variants carry the original error via `#[from]` so `?` works
/// directly. The `Custom` variant is a catch-all for business-logic errors that
/// are currently expressed as `format!(...)` strings — migrate these to domain
/// variants over time.
#[derive(Error, Debug)]
pub enum KeelError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("{0}")]
    Custom(String),
}

/// Blanket conversion from `String` so existing `.map_err(|e| format!(...))`
/// patterns continue to compile during the gradual migration. Callers that
/// still return `Result<T, String>` can use `?` on `KeelError` and vice versa.
impl From<String> for KeelError {
    fn from(source: String) -> Self {
        Self::Custom(source)
    }
}

/// Allow converting `&str` into `KeelError` for convenience in error paths that
/// start from a static message rather than a runtime `format!`.
impl From<&str> for KeelError {
    fn from(source: &str) -> Self {
        Self::Custom(source.to_owned())
    }
}

/// Reverse conversion so callers that still return `Result<T, String>` can use
/// `?` on `KeelError` results without rewriting their signature yet.
impl From<KeelError> for String {
    fn from(source: KeelError) -> Self {
        source.to_string()
    }
}
