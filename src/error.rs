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

    /// The launcher API rejected the request as from an outdated launcher
    /// (`ErrDeprecatedApi`). Update the RSI launcher.
    #[error("the launcher API reports this launcher version as outdated (ErrDeprecatedApi)")]
    LauncherOutdated,

    /// HTTP transport error (token mint, Spectrum).
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    /// A Spectrum route returned a non-success envelope.
    #[error("Spectrum {path} failed (code={code:?}, msg={msg:?})")]
    Spectrum {
        /// The route, e.g. `"/message/history"`.
        path: String,
        /// The envelope's `code`, e.g. `"ErrPermissionDenied"`.
        code: Option<String>,
        /// The envelope's `msg`.
        msg: Option<String>,
    },

    /// Spectrum's per-IP, per-route rate limit (`ErrThrottleLimit`; ~1 req/s).
    /// Clears within about two minutes — back off, don't retry immediately.
    #[error("Spectrum {path} throttled (ErrThrottleLimit) — back off before retrying")]
    Throttled {
        /// The throttled route.
        path: String,
    },

    /// gRPC channel/transport error (connect, TLS, …).
    #[error("gRPC transport error: {0}")]
    Transport(#[from] tonic::transport::Error),

    /// A gRPC call returned a non-OK status.
    #[error("gRPC call failed: {0}")]
    Rpc(#[from] tonic::Status),

    /// A value couldn't be parsed/validated (endpoint URL, JWT header, …).
    #[error("invalid value: {0}")]
    Invalid(String),

    /// `IdentityService.GetCurrentPlayer` returned no active character (or an
    /// empty player JWT), so the player-sharded queries (`items`,
    /// `entitlements`) can't run. Create/select a character in-game and retry.
    #[error("no current player/character for this account")]
    NoCurrentPlayer,
}
