//! The pledge store's ship list — the public RSI website's store GraphQL,
//! anonymous.
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! use sc_dossier::pledge::PledgeStore;
//! for s in PledgeStore::new("my-app/1.0").ships().await? {
//!     println!("{:?} {} {:?}", s.published_at, s.name, s.msrp_cents);
//! }
//! # Ok(()) }
//! ```
//!
//! [`PledgeStore::ships`] is what `/en/pledge/ships` lists: every ship and
//! vehicle, flight-ready and in concept, newest store page first. One
//! `POST /graphql` per 30 ships (the server's page cap); about 9 calls today.
//! Sorted by `publish_start`, paging is stable — the default sort repeats rows
//! across pages.
//!
//! Ids stay wire-native: [`StoreShip::id`] is the ship-matrix id.
//!
//! No pacing, caching, or retries — the caller's policy.

use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};

/// Production host.
pub const PROD_ROOT: &str = "https://robertsspaceindustries.com";

/// Ships per page; the server caps `limit` here.
const PAGE_SIZE: u32 = 30;

const SHIPS_QUERY: &str = "query Ships($query: SearchQuery) { \
    store(name: \"pledge\", browse: true) { search(query: $query) { totalCount \
    resources { ... on RSIShip { id name url msrp publishStart \
    media { thumbnail { storeSmall slideshow wallpaperMedium } } } } } } }";

/// Anonymous client for the pledge store.
#[derive(Debug, Clone)]
pub struct PledgeStore {
    http: reqwest::Client,
    root: String,
    user_agent: String,
}

impl PledgeStore {
    /// A client against [`PROD_ROOT`]; `user_agent` goes on every request.
    pub fn new(user_agent: &str) -> Self {
        Self {
            http: reqwest::Client::new(),
            root: PROD_ROOT.to_string(),
            user_agent: user_agent.to_string(),
        }
    }

    /// Use another host (tests, mirrors). Defaults to [`PROD_ROOT`].
    pub fn with_root(mut self, root: &str) -> Self {
        self.root = root.trim_end_matches('/').to_string();
        self
    }

    /// Every ship in the store, newest store page first (undated ones last).
    /// Walks all pages.
    pub async fn ships(&self) -> Result<Vec<StoreShip>> {
        let mut out: Vec<StoreShip> = Vec::new();
        for page in 1.. {
            let (ships, total) = self.page(page).await?;
            if ships.is_empty() {
                break;
            }
            for s in ships {
                if !out.iter().any(|o| o.id == s.id) {
                    out.push(s);
                }
            }
            // A server that keeps answering stops here, not never.
            if out.len() >= total || page > total as u32 / PAGE_SIZE + 1 {
                break;
            }
        }
        Ok(out)
    }

    async fn page(&self, page: u32) -> Result<(Vec<StoreShip>, usize)> {
        let url = format!("{}/graphql", self.root);
        let body = json!({
            "operationName": "Ships",
            "query": SHIPS_QUERY,
            "variables": { "query": {
                "page": page,
                "limit": PAGE_SIZE,
                "ships": { "all": true },
                "sort": { "field": "publish_start", "direction": "desc" },
            }},
        });
        let resp = self
            .http
            .post(&url)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(page_err(&url, &format!("HTTP {status}")));
        }
        let env: Value = resp
            .json()
            .await
            .map_err(|_| page_err(&url, "reply is not JSON"))?;
        parse_page(&env, &self.root).map_err(|what| page_err(&url, &what))
    }
}

fn page_err(url: &str, what: &str) -> Error {
    Error::Page {
        url: url.to_string(),
        what: what.to_string(),
    }
}

/// A ship or vehicle as the store lists it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoreShip {
    /// Ship-matrix id (`326`). Not a release order.
    pub id: u32,
    /// Display name (`"Constellation Mk V Centaurus"`).
    pub name: String,
    /// Absolute store page URL.
    pub url: String,
    /// Standalone price in US cents (`4500` = $45), the store's `msrp`. `None`
    /// on a few legacy rows (F7A Hornet Mk II).
    pub msrp_cents: Option<u32>,
    /// When the store page was published (`publishStart`; timezone unstated).
    ///
    /// **Unreliable as a release date.** It is roughly the first (concept)
    /// sale, not when the ship became flyable: Idris-M reads 2012-10-10, the
    /// Kickstarter, and the Merchantman 2013 though it is still in concept. 13
    /// early ships share the backfill date 2014-12-19, and a few rows (Expanse,
    /// Zeus Mk II) have none.
    pub published_at: Option<NaiveDateTime>,
    /// Image URLs.
    pub media: ShipMedia,
}

/// A store ship's images on `media.robertsspaceindustries.com`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ShipMedia {
    /// Store-card thumbnail (`store_small.jpg`).
    pub thumbnail: Option<String>,
    /// Slideshow size (`slideshow.jpg`).
    pub slideshow: Option<String>,
    /// 1920×1080 wallpaper (`wallpaper_1920x1080.jpg`).
    pub wallpaper: Option<String>,
}

/// `{data: {store: {search: {totalCount, resources}}}}` → ships + total.
/// `Err` names what was wrong; GraphQL errors carry their message.
fn parse_page(env: &Value, root: &str) -> std::result::Result<(Vec<StoreShip>, usize), String> {
    if let Some(errs) = env.get("errors").and_then(Value::as_array) {
        let msgs: Vec<&str> = errs
            .iter()
            .filter_map(|e| e.get("message").and_then(Value::as_str))
            .collect();
        return Err(format!("GraphQL error: {}", msgs.join("; ")));
    }
    let search = env
        .pointer("/data/store/search")
        .ok_or("no data.store.search")?;
    let total = search
        .get("totalCount")
        .and_then(Value::as_u64)
        .ok_or("no totalCount")? as usize;
    let ships = search
        .get("resources")
        .and_then(Value::as_array)
        .ok_or("no resources")?
        .iter()
        .filter_map(|r| parse_ship(r, root))
        .collect();
    Ok((ships, total))
}

/// One `RSIShip`; `None` if it has no id or name (a non-ship resource).
fn parse_ship(r: &Value, root: &str) -> Option<StoreShip> {
    let str_of = |v: Option<&Value>| v.and_then(Value::as_str).map(str::to_string);
    let id = match r.get("id")? {
        Value::String(s) => s.parse().ok()?,
        v => u32::try_from(v.as_u64()?).ok()?,
    };
    let url = str_of(r.get("url")).unwrap_or_default();
    let url = if url.starts_with('/') {
        format!("{root}{url}")
    } else {
        url
    };
    let thumb = r.pointer("/media/thumbnail");
    let image = |key| str_of(thumb.and_then(|t| t.get(key)));
    Some(StoreShip {
        id,
        name: str_of(r.get("name"))?,
        url,
        msrp_cents: r
            .get("msrp")
            .and_then(Value::as_u64)
            .and_then(|c| u32::try_from(c).ok()),
        published_at: r
            .get("publishStart")
            .and_then(Value::as_str)
            .and_then(parse_datetime),
        media: ShipMedia {
            thumbnail: image("storeSmall"),
            slideshow: image("slideshow"),
            wallpaper: image("wallpaperMedium"),
        },
    })
}

/// `"2026-10-14 16:00:00"`.
fn parse_datetime(s: &str) -> Option<NaiveDateTime> {
    let (date, time) = s.trim().split_once(' ')?;
    let mut d = date.split('-').map(str::parse::<u32>);
    let mut t = time.split(':').map(str::parse::<u32>);
    let (y, m, day) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let (h, min, sec) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    if d.next().is_some() || t.next().is_some() {
        return None;
    }
    NaiveDate::from_ymd_opt(i32::try_from(y).ok()?, m, day)?.and_hms_opt(h, min, sec)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIRST: &str = include_str!("../tests/fixtures/pledge/ships_p1.json");
    const LAST: &str = include_str!("../tests/fixtures/pledge/ships_p9.json");
    const PAST_END: &str = include_str!("../tests/fixtures/pledge/ships_p10.json");

    fn page(raw: &str) -> (Vec<StoreShip>, usize) {
        parse_page(&serde_json::from_str(raw).unwrap(), PROD_ROOT).unwrap()
    }

    fn at(y: i32, m: u32, d: u32, h: u32) -> Option<NaiveDateTime> {
        NaiveDate::from_ymd_opt(y, m, d)?.and_hms_opt(h, 0, 0)
    }

    #[test]
    fn parses_ships() {
        let (ships, total) = page(FIRST);
        assert_eq!((ships.len(), total), (30, 256));
        let s = &ships[0];
        assert_eq!((s.id, s.name.as_str()), (326, "Skylark"));
        assert_eq!(
            s.url,
            "https://robertsspaceindustries.com/pledge/ships/skylark/Skylark"
        );
        assert_eq!(s.msrp_cents, Some(4500));
        assert_eq!(s.published_at, at(2026, 10, 14, 16));
        assert_eq!(
            s.media.thumbnail.as_deref(),
            Some("https://media.robertsspaceindustries.com/nxwjwdxucmknm/store_small.jpg")
        );
        assert!(s
            .media
            .wallpaper
            .as_deref()
            .unwrap()
            .ends_with("/wallpaper_1920x1080.jpg"));
        assert!(ships
            .windows(2)
            .all(|w| w[0].published_at >= w[1].published_at));
    }

    #[test]
    fn legacy_rows_have_gaps() {
        let (ships, _) = page(LAST);
        let by = |name: &str| ships.iter().find(|s| s.name == name).unwrap();
        assert_eq!(by("Idris-M").published_at, at(2012, 10, 10, 0));
        assert_eq!(by("F7A Hornet Mk II").msrp_cents, None);
        assert_eq!(by("Expanse").published_at, None);
        assert!(ships.iter().all(|s| s.media.thumbnail.is_some()));
    }

    #[test]
    fn past_the_end_is_empty() {
        assert_eq!(page(PAST_END), (vec![], 256));
    }

    #[test]
    fn graphql_errors_and_bad_shapes() {
        let err = parse_page(
            &json!({"errors": [{"message": "Internal server error", "code": "CFUException"}]}),
            PROD_ROOT,
        )
        .unwrap_err();
        assert!(err.contains("Internal server error"), "{err}");
        assert!(parse_page(&json!({"data": null}), PROD_ROOT).is_err());
        assert!(parse_page(&json!({"data": {"store": {"search": {}}}}), PROD_ROOT).is_err());
    }

    #[test]
    fn adversarial_input_does_not_panic() {
        for r in [
            json!({}),
            json!({"id": "x", "name": "a"}),
            json!({"id": "99999999999", "name": "a"}),
            json!({"id": -1, "name": "a"}),
            json!({"id": 1, "name": 5}),
            json!({"id": 1, "name": "a", "msrp": -5, "publishStart": "2026-13-40 99:00:00"}),
            json!({"id": 1, "name": "a", "media": {"thumbnail": "x"}, "url": 3}),
        ] {
            let _ = parse_ship(&r, PROD_ROOT);
        }
        for s in [
            "",
            "2026-10-14",
            "2026-10-14 16:00",
            "2026-10-14 16:00:00:00",
            "2026-02-30 00:00:00",
            "é-1-1 0:0:0",
        ] {
            assert_eq!(parse_datetime(s), None, "{s:?}");
        }
    }

    /// The whole list, live.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_ships() {
        let ships = PledgeStore::new("sc-dossier-test").ships().await.unwrap();
        assert!(ships.len() > 200, "{}", ships.len());
        let mut ids: Vec<u32> = ships.iter().map(|s| s.id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), ships.len());
        assert!(ships.iter().all(|s| !s.name.is_empty()));
    }
}
