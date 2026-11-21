//! Launcher v3 calls on the stored launcher [`Credentials`]: mint a
//! game-services [`Session`], list the account's [`library`].

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::store::Credentials;

/// Which server environment, and so which account data, to read. Each has
/// its own launcher API and sign-in session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Env {
    /// The live servers (`robertsspaceindustries.com`, the stored launcher
    /// session). Channel `LIVE`.
    Live,
    /// The PTU servers (`ptu.cloudimperiumgames.com`, a PTU session signed in
    /// from the live one). Channel `PTU`, else `EPTU`, whichever the account
    /// may play now.
    Ptu,
}

impl Env {
    fn api_base(self) -> &'static str {
        match self {
            Env::Live => "https://robertsspaceindustries.com/api/launcher/v3",
            Env::Ptu => "https://ptu.cloudimperiumgames.com/api/launcher/v3",
        }
    }
}

/// Options for the mint flow. [`MintOptions::from_credentials`] takes the
/// [`Env`] from the caller and the User-Agent from the launcher store.
#[derive(Debug, Clone)]
pub struct MintOptions {
    /// Server environment to mint for.
    pub env: Env,
    /// Game id. Defaults to `"SC"`.
    pub game: String,
    /// User-Agent for the launcher v3 calls.
    pub user_agent: String,
}

impl MintOptions {
    /// Options for `env`, UA from the launcher version. The channel selected
    /// in the launcher is not read: each environment has its own account
    /// data, so the caller picks it.
    ///
    /// The UA mirrors the launcher's own, `${app.getName()}-${app.getVersion()}`
    /// (e.g. `rsilauncher-2.18.1`).
    pub fn from_credentials(creds: &Credentials, env: Env) -> Self {
        Self {
            env,
            game: "SC".into(),
            user_agent: format!(
                "rsilauncher-{}",
                creds.launcher_version.as_deref().unwrap_or("2.18.1")
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

/// One platform's launcher API with its signed-in session.
struct Api<'a> {
    client: &'a reqwest::Client,
    env: Env,
    user_agent: &'a str,
    /// `X-Rsi-Token` on live, `X-Rsi-PTU-Token` on PTU; `None` before sign-in.
    session: Option<(String, String)>,
    device: &'a str,
}

impl<'a> Api<'a> {
    /// The live API on the stored launcher session.
    fn live(client: &'a reqwest::Client, creds: &'a Credentials, opts: &'a MintOptions) -> Self {
        Self {
            client,
            env: Env::Live,
            user_agent: &opts.user_agent,
            session: Some(("X-Rsi-Token".into(), creds.session_token.clone())),
            device: &creds.device_token,
        }
    }

    /// Sign in to `env` from this live session as the launcher does:
    /// `account/claims` here, `signin/claims` there.
    async fn sign_in(&self, env: Env) -> Result<Api<'a>> {
        let claims = self.call("account/claims", json!({})).await?;
        let mut other = Api {
            client: self.client,
            env,
            user_agent: self.user_agent,
            session: None,
            device: self.device,
        };
        let data = other
            .call("signin/claims", json!({ "claims": claims }))
            .await?;
        let field = |key: &str| {
            data.get(key)
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .ok_or_else(|| Error::Invalid(format!("signin/claims returned no {key}")))
        };
        other.session = Some((
            format!("X-{}", field("session_name")?),
            field("session_id")?,
        ));
        Ok(other)
    }

    async fn call(&self, step: &'static str, payload: Value) -> Result<Value> {
        let url = format!("{}/{step}", self.env.api_base());
        let mut req = self
            .client
            .post(&url)
            .header("User-Agent", self.user_agent)
            .header("Content-Type", "application/json")
            .header("X-Rsi-Device", self.device);
        if let Some((header, token)) = &self.session {
            req = req.header(header.as_str(), token);
        }
        let resp = req.json(&payload).send().await?;

        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("no content type")
            .to_string();
        // An error envelope may come with a 4xx status; only a non-JSON body
        // is unreadable.
        let envelope = if content_type.starts_with("application/json") {
            resp.json::<Envelope>().await.ok()
        } else {
            None
        };
        let Some(env) = envelope else {
            return Err(Error::LauncherHttp {
                step,
                status,
                content_type,
            });
        };
        if env.code.as_deref() == Some("ErrDeprecatedApi") {
            return Err(Error::LauncherOutdated);
        }
        if env.success != 1 {
            return Err(Error::Mint {
                step,
                success: env.success,
                code: env.code,
            });
        }
        Ok(env.data)
    }
}

/// Mint a [`Session`] for `opts.env`. `games/release` on the live API names
/// the channel's endpoint; the token comes from the environment's own API,
/// after signing in to it for [`Env::Ptu`]. Returns [`Error::EnvClosed`] if no
/// channel of that environment is open to this account.
pub async fn mint_session(creds: &Credentials, opts: &MintOptions) -> Result<Session> {
    let client = reqwest::Client::new();
    let live = Api::live(&client, creds, opts);

    let claims = live.call("games/claims", json!({})).await?;
    let channel = match opts.env {
        Env::Live => "LIVE".to_string(),
        Env::Ptu => {
            let data = live
                .call("games/library", json!({ "claims": claims }))
                .await?;
            let games = parse_library(&data)
                .ok_or_else(|| Error::Invalid("games/library: unexpected shape".into()))?;
            ptu_channel(&games, &opts.game).ok_or(Error::EnvClosed { env: "PTU" })?
        }
    };
    let release = live
        .call(
            "games/release",
            json!({ "claims": claims, "gameId": opts.game, "channelId": channel }),
        )
        .await?;

    let services_endpoint = release
        .get("servicesEndpoint")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid("games/release returned no servicesEndpoint".into()))?;
    let version_label = release
        .get("versionLabel")
        .and_then(Value::as_str)
        .map(str::to_string);

    let token_payload = |claims: &Value| json!({ "claims": claims, "gameId": opts.game });
    let token_data = match opts.env {
        Env::Live => live.call("games/token", token_payload(&claims)).await?,
        Env::Ptu => {
            let api = live.sign_in(Env::Ptu).await?;
            let claims = api.call("games/claims", json!({})).await?;
            api.call("games/token", token_payload(&claims)).await?
        }
    };
    let jwt = token_data
        .get("token")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .ok_or_else(|| Error::Invalid("games/token returned no token".into()))?;

    Ok(Session {
        jwt,
        services_endpoint,
        version_label,
    })
}

/// The PTU-servers channel of `game` to mint on: `PTU`, else `EPTU`, else any
/// other channel on a PTU platform.
fn ptu_channel(games: &[Game], game: &str) -> Option<String> {
    let channels: Vec<&GameChannel> = games
        .iter()
        .find(|g| g.id == game)?
        .channels
        .iter()
        .filter(|c| matches!(c.platform_id.as_str(), "ptu" | "eptu"))
        .collect();
    ["PTU", "EPTU"]
        .iter()
        .find_map(|id| channels.iter().find(|c| c.id == *id))
        .or(channels.first())
        .map(|c| c.id.clone())
}

/// The games and channels **this account** may play — what the launcher's
/// game selector shows (`games/claims` → `games/library` on the live API;
/// `opts.env` and `opts.game` are unused).
///
/// Account-scoped: a test channel (PTU, EPTU, HOTFIX, TECH-PREVIEW) appears
/// only while open *and* open to this account. The API answers a closed
/// channel and one this account can't play alike.
pub async fn library(creds: &Credentials, opts: &MintOptions) -> Result<Vec<Game>> {
    let client = reqwest::Client::new();
    let live = Api::live(&client, creds, opts);
    let claims = live.call("games/claims", json!({})).await?;
    let data = live
        .call("games/library", json!({ "claims": claims }))
        .await?;
    parse_library(&data).ok_or_else(|| Error::Invalid("games/library: unexpected shape".into()))
}

/// A game in the account's [`library`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Game {
    /// `"SC"`.
    pub id: String,
    /// `"Star Citizen"`.
    pub name: String,
    /// Game status code (`"A"` = available).
    pub status: String,
    /// Sort weight.
    pub weight: i64,
    /// Playable without the backend.
    pub playable_offline: bool,
    /// Has an installable package.
    pub has_game_package: bool,
    /// Promoted in the launcher.
    pub is_promoted: bool,
    /// `playTime` as sent (`{lastGameplay, pubTimePlayed}`, null so far).
    pub play_time: Value,
    /// Channels this account may play, launcher order.
    pub channels: Vec<GameChannel>,
}

/// A release channel of a [`Game`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameChannel {
    /// `"LIVE"`, `"PTU"`, `"EPTU"`, `"HOTFIX"`, `"TECH-PREVIEW"`, ….
    pub id: String,
    /// Display name, e.g. `"Live Release"`.
    pub name: String,
    /// Client build (changelist), e.g. `12913470`.
    pub version: u64,
    /// E.g. `"4.10.2-live.12913470"`.
    pub version_label: String,
    /// Launcher platform: `"prod"`, `"ptu"`.
    pub platform_id: String,
    /// Game-services endpoint; its host names the *server* build
    /// (`pub-sc-alpha-4102-12881860…`).
    pub services_endpoint: String,
    /// Install folder name, e.g. `"StarCitizen"`.
    pub install_dir: String,
    /// Sort weight.
    pub weight: i64,
    /// `network` as sent (`{mesh: {channels: {…}}}`; null on LIVE).
    pub network: Value,
}

fn parse_library(data: &Value) -> Option<Vec<Game>> {
    data.get("games")?
        .as_array()?
        .iter()
        .map(|g| {
            // `channels` is an array as served; the launcher's
            // `Object.values` also accepts an object keyed by channel.
            let channels: Vec<&Value> = match g.get("channels") {
                Some(Value::Array(a)) => a.iter().collect(),
                Some(Value::Object(o)) => o.values().collect(),
                _ => Vec::new(),
            };
            Some(Game {
                id: string(g, "id")?,
                name: string(g, "name").unwrap_or_default(),
                status: string(g, "status").unwrap_or_default(),
                weight: number(g, "weight").unwrap_or(0),
                playable_offline: flag(g, "playableOffline"),
                has_game_package: flag(g, "hasGamePackage"),
                is_promoted: flag(g, "isPromoted"),
                play_time: g.get("playTime").cloned().unwrap_or(Value::Null),
                channels: channels
                    .into_iter()
                    .filter_map(|c| {
                        Some(GameChannel {
                            id: string(c, "id")?,
                            name: string(c, "name").unwrap_or_default(),
                            version: number(c, "version")
                                .and_then(|n| u64::try_from(n).ok())
                                .unwrap_or(0),
                            version_label: string(c, "versionLabel").unwrap_or_default(),
                            platform_id: string(c, "platformId").unwrap_or_default(),
                            services_endpoint: string(c, "servicesEndpoint").unwrap_or_default(),
                            install_dir: string(c, "installDir").unwrap_or_default(),
                            weight: number(c, "weight").unwrap_or(0),
                            network: c.get("network").cloned().unwrap_or(Value::Null),
                        })
                    })
                    .collect(),
            })
        })
        .collect()
}

fn string(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

/// A number, or a numeric string (the launcher runs `Number(…)` on weights).
fn number(v: &Value, key: &str) -> Option<i64> {
    match v.get(key)? {
        Value::Number(n) => n.as_i64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIBRARY: &str = include_str!("../tests/fixtures/launcher/library.json");

    #[test]
    fn parses_library() {
        let env: Value = serde_json::from_str(LIBRARY).unwrap();
        let games = parse_library(&env["data"]).unwrap();
        assert_eq!(games.len(), 1);
        let g = &games[0];
        assert_eq!(
            (g.id.as_str(), g.name.as_str(), g.status.as_str()),
            ("SC", "Star Citizen", "A")
        );
        assert!(g.has_game_package && g.is_promoted && !g.playable_offline);
        assert_eq!(g.channels.len(), 1);
        let c = &g.channels[0];
        assert_eq!(c.id, "LIVE");
        assert_eq!(c.name, "Live Release");
        assert_eq!(c.version, 12913470);
        assert_eq!(c.version_label, "4.10.2-live.12913470");
        assert_eq!(c.platform_id, "prod");
        assert_eq!(c.install_dir, "StarCitizen");
        assert_eq!(c.weight, 10);
        assert!(c.services_endpoint.contains("pub-sc-alpha-4102-"));
        assert!(c.network.is_null());
    }

    #[test]
    fn library_channels_as_object_and_bad_shapes() {
        let data = json!({"games": [{"id": "SC", "weight": "3", "channels": {
            "PTU": {"id": "PTU", "version": 12939222, "platformId": "ptu", "weight": "20"}}}]});
        let g = &parse_library(&data).unwrap()[0];
        assert_eq!(
            (g.weight, g.channels[0].id.as_str(), g.channels[0].weight),
            (3, "PTU", 20)
        );
        assert!(parse_library(&json!({})).is_none());
        assert!(parse_library(&json!({"games": [{"name": "no id"}]})).is_none());
    }

    #[test]
    fn picks_ptu_channel() {
        let lib = |chs: Value| parse_library(&json!({"games": [{"id": "SC", "channels": chs}]}));
        let ch = |id: &str, p: &str| json!({"id": id, "platformId": p});
        let both = lib(json!([
            ch("LIVE", "prod"),
            ch("EPTU", "eptu"),
            ch("PTU", "ptu")
        ]))
        .unwrap();
        assert_eq!(ptu_channel(&both, "SC").as_deref(), Some("PTU"));
        let eptu = lib(json!([ch("LIVE", "prod"), ch("EPTU", "eptu")])).unwrap();
        assert_eq!(ptu_channel(&eptu, "SC").as_deref(), Some("EPTU"));
        let live = lib(json!([ch("LIVE", "prod"), ch("HOTFIX", "prod")])).unwrap();
        assert_eq!(ptu_channel(&live, "SC"), None);
    }

    /// The account's library, live (needs a signed-in RSI launcher).
    #[tokio::test]
    #[ignore = "live: needs the launcher session"]
    async fn live_library() {
        let creds = crate::store::read_credentials().unwrap();
        let games = library(&creds, &MintOptions::from_credentials(&creds, Env::Live))
            .await
            .unwrap();
        let sc = games.iter().find(|g| g.id == "SC").unwrap();
        assert!(sc.channels.iter().any(|c| c.id == "LIVE" && c.version > 0));
    }
}
