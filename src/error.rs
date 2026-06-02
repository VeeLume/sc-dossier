//! Error type for the crate.

use std::path::PathBuf;

/// Convenience alias for results in this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Errors from credential reading, token minting, and backend queries.
///
/// `#[non_exhaustive]`: match with a wildcard arm; new variants may be added in
/// minor releases.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The RSI Launcher install (and thus its `app.asar` decryption key) could
    /// not be located.
    #[error("RSI Launcher installation not found (is it installed?)")]
    LauncherNotFound,

    /// The launcher store file doesn't exist — usually means the user has never
    /// signed in on this machine.
    #[error("launcher store not found at {0} — open the RSI launcher and sign in")]
    StoreNotFound(PathBuf),

    /// Reading or decrypting the launcher store failed (I/O, bad format, key
    /// rotation, …).
    #[error("reading/decrypting the launcher store failed: {0}")]
    Store(String),

    /// The store decrypted but a required credential field was missing.
    #[error("launcher store has no {what} — open the RSI launcher and sign in")]
    MissingCredential {
        /// Which field was missing (e.g. `"session token"`).
        what: &'static str,
    },

    /// A token-mint step returned a non-success envelope (e.g. an expired
    /// session). `code` is CIG's error code when present.
    #[error(
        "token mint step '{step}' failed (success={success}, code={code:?}). \
         A stale session reports here — re-open the RSI launcher to refresh it."
    )]
    Mint {
        /// The launcher v3 endpoint that failed (`games/claims` | `games/release` | `games/token`).
        step: &'static str,
        /// The envelope's `success` field (1 = ok).
        success: i64,
        /// The envelope's `code` field, if any.
        code: Option<String>,
    },

    /// HTTP transport error during the mint flow.
    #[error("HTTP error during token mint: {0}")]
    Http(#[from] reqwest::Error),

    /// gRPC channel/transport error (connect, TLS, …).
    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// A gRPC call returned a non-OK status.
    #[error("gRPC call failed: {0}")]
    Rpc(#[from] tonic::Status),

    /// A value couldn't be parsed/validated (endpoint URL, JWT header, …).
    #[error("invalid value: {0}")]
    Invalid(String),
}
