//! Mint a game-services JWT by replaying the launcher flow:
//!
//!   1. `POST games/claims`  `{}`                          → opaque `claims`
//!   2. `POST games/release` `{claims, gameId, channelId}` → `servicesEndpoint`, version
//!   3. `POST games/token`   `{claims, gameId}`            → game JWT
//!
//! All three send `X-Rsi-Token` + `X-Rsi-Device` as headers. Envelope is
//! `{ success, data, code }`; `success != 1` → [`Error::Mint`].

use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::store::Credentials;

/// RSI environment to mint against. Maps to the launcher v3 API base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Env {
    /// Live (`robertsspaceindustries.com`).
    Prod,
    /// PTU / EPTU (`ptu.cloudimperiumgames.com`).
    Ptu,
    /// Staging.
    Staging,
}

impl Env {
    fn api_base(self) -> &'static str {
        match self {
            Env::Prod => "https://robertsspaceindustries.com/api/launcher/v3",
            Env::Ptu => "https://ptu.cloudimperiumgames.com/api/launcher/v3",
            Env::Staging => "https://staging.cloudimperiumgames.com/api/launcher/v3",
        }
    }

    /// Map a launcher `platformId` (`"prod"` / `"ptu"`) to an [`Env`].
    pub fn from_platform_id(platform_id: Option<&str>) -> Env {
        match platform_id {
            Some("ptu") => Env::Ptu,
            _ => Env::Prod,
        }
    }
}

/// Options for the mint flow. [`MintOptions::from_credentials`] derives
/// env / channel / User-Agent from the launcher store.
#[derive(Debug, Clone)]
pub struct MintOptions {
    /// Environment to mint against.
    pub env: Env,
    /// Game id. Defaults to `"SC"`.
    pub game: String,
    /// Channel id, e.g. `"LIVE"`.
    pub channel: String,
    /// User-Agent for the launcher v3 calls.
    pub user_agent: String,
}

impl MintOptions {
    /// Derive options from launcher credentials (env from platform id, channel
    /// from the store default, UA from the launcher version).
    pub fn from_credentials(creds: &Credentials) -> Self {
        Self {
            env: Env::from_platform_id(creds.platform_id.as_deref()),
            game: "SC".into(),
            channel: creds.default_channel.clone().unwrap_or_else(|| "LIVE".into()),
            user_agent: format!(
                "RSI Launcher/{}",
                creds.launcher_version.as_deref().unwrap_or("2.13.3")
            ),
        }
    }
}

/// A minted game session: JWT plus services endpoint.
#[derive(Clone)]
pub struct Session {
    /// Account-scoped game JWT. Secret — redacted in `Debug`.
    pub jwt: String,
    /// Services endpoint, e.g.
    /// `https://pub-sc-alpha-480-11825000.test1.cloudimperiumgames.com:443`.
    pub services_endpoint: String,
    /// Launcher version label, e.g. `"4.8.0-live.11875683"`, when present.
    pub version_label: Option<String>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("jwt", &format_args!("<redacted, {} bytes>", self.jwt.len()))
            .field("services_endpoint", &self.services_endpoint)
            .field("version_label", &self.version_label)
            .finish()
    }
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(default)]
    success: i64,
    #[serde(default)]
    data: Value,
    #[serde(default)]
    code: Option<String>,
}

async fn call(
    client: &reqwest::Client,
    creds: &Credentials,
    opts: &MintOptions,
    step: &'static str,
    payload: Value,
) -> Result<Value> {
    let url = format!("{}/{step}", opts.env.api_base());
    let resp = client
        .post(&url)
        .header("User-Agent", &opts.user_agent)
        .header("Content-Type", "application/json")
        .header("X-Rsi-Token", &creds.session_token)
        .header("X-Rsi-Device", &creds.device_token)
        .json(&payload)
        .send()
        .await?;

    let env: Envelope = resp.json().await?;
    if env.success != 1 {
        return Err(Error::Mint { step, success: env.success, code: env.code });
    }
    Ok(env.data)
}

/// Run the three-step mint flow and return a [`Session`].
pub async fn mint_session(creds: &Credentials, opts: &MintOptions) -> Result<Session> {
    let client = reqwest::Client::new();

    let claims = call(&client, creds, opts, "games/claims", json!({})).await?;

    let release = call(
        &client,
        creds,
        opts,
        "games/release",
        json!({ "claims": claims, "gameId": opts.game, "channelId": opts.channel }),
    )
    .await?;

    let services_endpoint = release
        .get("servicesEndpoint")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid("games/release returned no servicesEndpoint".into()))?;
    let version_label = release.get("versionLabel").and_then(Value::as_str).map(str::to_string);

    let token_data = call(
        &client,
        creds,
        opts,
        "games/token",
        json!({ "claims": claims, "gameId": opts.game }),
    )
    .await?;
    let jwt = token_data
        .get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid("games/token returned no token".into()))?;

    Ok(Session { jwt, services_endpoint, version_label })
}
