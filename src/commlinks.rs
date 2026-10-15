//! Comm-links — RSI's news posts — from the public website's listing,
//! anonymous.
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! use sc_dossier::commlinks::{CommLinkQuery, CommLinks};
//! let page = CommLinks::new("my-app/1.0").page(&CommLinkQuery::default()).await?;
//! for c in &page.items {
//!     println!("{} [{:?}] {}", c.age_text, c.kind, c.title);
//! }
//! # Ok(()) }
//! ```
//!
//! One [`CommLinks::page`] call is one `POST /api/hub/getCommlinkItems`: 10
//! cards, newest first unless asked otherwise. The listing carries **no
//! absolute dates** — only a relative age ("11 hours ago", "3 years ago").
//!
//! No pacing, caching, or retries — the caller's policy. The listing's rate
//! limit is unmeasured; stay under ~1 request/second.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::html::{between, decode_entities, squash, text};

/// Production host.
pub const PROD_ROOT: &str = "https://robertsspaceindustries.com";

/// Channel slugs the website filters by.
pub mod channel {
    /// News, ship sales, events.
    pub const TRANSMISSION: &str = "transmission";
    /// Q&As, dev posts.
    pub const ENGINEERING: &str = "engineering";
    /// Community content.
    pub const CITIZENS: &str = "citizens";
    /// Lore.
    pub const SPECTRUM_DISPATCH: &str = "spectrum-dispatch";
    /// Serialized fiction.
    pub const SERIALIZED_FICTION: &str = "serialized-fiction";
}

/// Anonymous client for the comm-link listing.
#[derive(Debug, Clone)]
pub struct CommLinks {
    http: reqwest::Client,
    root: String,
    user_agent: String,
}

impl CommLinks {
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

    /// One listing page.
    pub async fn page(&self, query: &CommLinkQuery) -> Result<CommLinkPage> {
        let url = format!("{}/api/hub/getCommlinkItems", self.root);
        let resp = self
            .http
            .post(&url)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .json(&query.to_json())
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
        parse_page(&env, &self.root).map_err(|what| page_err(&url, what))
    }
}

fn page_err(url: &str, what: &str) -> Error {
    Error::Page {
        url: url.to_string(),
        what: what.to_string(),
    }
}

/// Listing filters. `Default` = all comm-links, newest first, page 1.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommLinkQuery {
    /// 1-based.
    pub page: u32,
    /// A [`channel`] slug.
    pub channel: Option<String>,
    /// A series slug, e.g. `"this-week-in-sc"`, `"roadmap-roundup"`,
    /// `"monthly-report"`, `"live"`, `"inside"`.
    pub series: Option<String>,
    /// Only this kind.
    pub kind: Option<CommLinkKind>,
    /// Free-text search.
    pub text: Option<String>,
    /// Oldest first instead of newest first.
    pub oldest_first: bool,
}

impl Default for CommLinkQuery {
    fn default() -> Self {
        Self {
            page: 1,
            channel: None,
            series: None,
            kind: None,
            text: None,
            oldest_first: false,
        }
    }
}

impl CommLinkQuery {
    /// The request body the website's hub page sends.
    fn to_json(&self) -> Value {
        json!({
            "channel": self.channel.as_deref().unwrap_or_default(),
            "series": self.series.as_deref().unwrap_or_default(),
            "type": self.kind.as_ref().map(CommLinkKind::slug).unwrap_or_default(),
            "text": self.text.as_deref().unwrap_or_default(),
            "sort": if self.oldest_first { "publish_old" } else { "publish_new" },
            "page": self.page.max(1),
        })
    }
}

/// One listing page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommLinkPage {
    /// Up to 10.
    pub items: Vec<CommLink>,
    /// No cards on this page — past the end of the listing.
    pub is_last: bool,
}

/// A comm-link card.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommLink {
    /// Comm-link id (`21381`), from the URL — stable per post, a safe key.
    /// Not publish order: drafts keep their id.
    pub id: u64,
    /// Absolute URL.
    pub url: String,
    /// Card title.
    pub title: String,
    /// Post, video, …
    pub kind: CommLinkKind,
    /// Channel slug from the URL path (`"transmission"`, `"engineering"`).
    pub channel: Option<String>,
    /// Summary, text only (may be empty).
    pub summary: String,
    /// ~30 KB `heap_infobox` variant of the card image.
    pub thumbnail: Option<String>,
    /// Comment count as shown.
    pub comments: Option<u32>,
    /// **Relative** age as shown ("11 hours ago").
    pub age_text: String,
    /// [`CommLink::age_text`] parsed; `None` if it doesn't parse.
    pub age: Option<Age>,
}

/// Card type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CommLinkKind {
    /// Article.
    Post,
    /// Video.
    Video,
    /// Image gallery.
    Slideshow,
    /// Poll.
    Poll,
    /// Anything else, by its slug.
    Other(String),
}

impl CommLinkKind {
    fn from_slug(s: &str) -> Self {
        match s {
            "post" => Self::Post,
            "video" => Self::Video,
            "slideshow" => Self::Slideshow,
            "poll" => Self::Poll,
            other => Self::Other(other.to_string()),
        }
    }

    fn slug(&self) -> &str {
        match self {
            Self::Post => "post",
            Self::Video => "video",
            Self::Slideshow => "slideshow",
            Self::Poll => "poll",
            Self::Other(s) => s,
        }
    }
}

/// A coarse relative age: "11 hours ago" → `{ value: 11, unit: Hour }`;
/// "just now" → `{ value: 0, unit: Second }`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Age {
    /// How many units.
    pub value: u32,
    /// Unit.
    pub unit: AgeUnit,
}

/// Unit of an [`Age`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AgeUnit {
    /// Seconds.
    Second,
    /// Minutes.
    Minute,
    /// Hours.
    Hour,
    /// Days.
    Day,
    /// Weeks.
    Week,
    /// Months.
    Month,
    /// Years.
    Year,
}

impl Age {
    /// Rough length in seconds (month = 30 days, year = 365 days) — for
    /// sorting or "older than", not for display.
    pub fn approx_seconds(&self) -> u64 {
        let unit: u64 = match self.unit {
            AgeUnit::Second => 1,
            AgeUnit::Minute => 60,
            AgeUnit::Hour => 3_600,
            AgeUnit::Day => 86_400,
            AgeUnit::Week => 7 * 86_400,
            AgeUnit::Month => 30 * 86_400,
            AgeUnit::Year => 365 * 86_400,
        };
        u64::from(self.value) * unit
    }
}

/// "just now", "11 hours ago", "1 day ago", "a month ago", "a few seconds ago".
fn parse_age(s: &str) -> Option<Age> {
    let s = s.trim().to_ascii_lowercase();
    if s == "just now" || s == "now" {
        return Some(Age {
            value: 0,
            unit: AgeUnit::Second,
        });
    }
    let s = s.replace("a few ", "0 ");
    let mut w = s.split_whitespace();
    let value = match w.next()? {
        "a" | "an" => 1,
        n => n.parse().ok()?,
    };
    let unit = match w.next()?.trim_end_matches('s') {
        "second" => AgeUnit::Second,
        "minute" => AgeUnit::Minute,
        "hour" => AgeUnit::Hour,
        "day" => AgeUnit::Day,
        "week" => AgeUnit::Week,
        "month" => AgeUnit::Month,
        "year" => AgeUnit::Year,
        _ => return None,
    };
    Some(Age { value, unit })
}

/// `{success, data: <HTML of cards>}` → page. `Err` names what was wrong.
fn parse_page(env: &Value, root: &str) -> std::result::Result<CommLinkPage, &'static str> {
    if env.get("success").and_then(Value::as_i64) != Some(1) {
        return Err("envelope success != 1");
    }
    let html = env
        .get("data")
        .and_then(Value::as_str)
        .ok_or("envelope has no HTML data")?;
    let items: Vec<CommLink> = html
        .split("<a class=\"content-block2")
        .skip(1)
        .filter_map(|card| parse_card(card, root))
        .collect();
    Ok(CommLinkPage {
        is_last: items.is_empty(),
        items,
    })
}

fn parse_card(card: &str, root: &str) -> Option<CommLink> {
    let href = decode_entities(between(card, "href=\"", "\"")?);
    let url = if href.starts_with('/') {
        format!("{root}{href}")
    } else {
        href
    };
    // …/comm-link/<channel>/<id>-<slug>
    let mut path = url.trim_end_matches('/').rsplit('/');
    let id = path.next()?.split('-').next()?.parse().ok()?;
    let channel = path
        .next()
        .filter(|c| *c != "comm-link")
        .map(str::to_string);
    let title = between(card, "class=\"title ", "</div>")
        .and_then(|t| t.split_once('>'))
        .map(|(_, t)| text(t))
        .unwrap_or_default();
    let age_text = between(card, "class=\"value\">", "<")
        .map(squash)
        .unwrap_or_default();
    Some(CommLink {
        id,
        url,
        title,
        kind: CommLinkKind::from_slug(between(card, "class=\"type ", "\"").unwrap_or_default()),
        channel,
        summary: between(card, "class=\"body\">", "</div>")
            .map(text)
            .unwrap_or_default(),
        thumbnail: between(card, "background-image:url('", "'")
            .map(|u| u.replace("/channel_item_full.", "/heap_infobox.")),
        comments: between(card, "class=\"comments\">", "<").and_then(|c| c.trim().parse().ok()),
        age: parse_age(&age_text),
        age_text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE1: &str = include_str!("../tests/fixtures/commlinks/page1.json");
    const END: &str = include_str!("../tests/fixtures/commlinks/end.json");

    fn page(raw: &str) -> CommLinkPage {
        parse_page(&serde_json::from_str(raw).unwrap(), PROD_ROOT).unwrap()
    }

    #[test]
    fn parses_cards() {
        let p = page(PAGE1);
        assert!(!p.is_last);
        assert_eq!(p.items.len(), 10);
        let c = &p.items[0];
        assert_eq!(c.id, 21381);
        assert_eq!(c.kind, CommLinkKind::Video);
        assert_eq!(c.channel.as_deref(), Some(channel::TRANSMISSION));
        assert_eq!(
            c.title,
            "Star Citizen | Behind the Ships: RSI Constellation Mk V Centaurus : Video"
        );
        assert!(c
            .url
            .starts_with("https://robertsspaceindustries.com/comm-link/transmission/21381-"));
        assert!(c.summary.starts_with("It’s the Conny"));
        assert!(
            c.summary.contains("‘versatile!’"),
            "entities decoded: {}",
            c.summary
        );
        assert_eq!(
            c.thumbnail.as_deref(),
            Some("https://media.robertsspaceindustries.com/m0izy86vfvs1j/heap_infobox.jpg")
        );
        assert_eq!(c.comments, Some(0));
        assert_eq!(c.age_text, "11 hours ago");
        assert_eq!(
            c.age,
            Some(Age {
                value: 11,
                unit: AgeUnit::Hour
            })
        );
        let q = &p.items[1];
        assert_eq!(
            (q.kind.clone(), q.channel.as_deref()),
            (CommLinkKind::Post, Some("engineering"))
        );
        assert!(p
            .items
            .iter()
            .all(|c| !c.title.is_empty() && c.age.is_some()));
    }

    #[test]
    fn end_of_listing() {
        let p = page(END);
        assert!(p.is_last);
        assert!(p.items.is_empty());
    }

    #[test]
    fn bad_envelopes() {
        assert!(parse_page(&json!({"success": 0, "code": "ErrX"}), PROD_ROOT).is_err());
        assert!(parse_page(&json!({"success": 1, "data": null}), PROD_ROOT).is_err());
    }

    #[test]
    fn query_matches_the_hub_page() {
        let q = CommLinkQuery {
            page: 3,
            channel: Some(channel::ENGINEERING.into()),
            kind: Some(CommLinkKind::Video),
            oldest_first: true,
            ..Default::default()
        };
        assert_eq!(
            q.to_json(),
            json!({"channel": "engineering", "series": "", "type": "video", "text": "",
                   "sort": "publish_old", "page": 3})
        );
        assert_eq!(CommLinkQuery::default().to_json()["sort"], "publish_new");
    }

    #[test]
    fn ages() {
        let age = |value, unit| Some(Age { value, unit });
        for (s, want) in [
            ("just now", age(0, AgeUnit::Second)),
            ("a few seconds ago", age(0, AgeUnit::Second)),
            ("1 second ago", age(1, AgeUnit::Second)),
            ("1 minute ago", age(1, AgeUnit::Minute)),
            ("45 minutes ago", age(45, AgeUnit::Minute)),
            ("an hour ago", age(1, AgeUnit::Hour)),
            ("11 hours ago", age(11, AgeUnit::Hour)),
            ("1 day ago", age(1, AgeUnit::Day)),
            ("2 days ago", age(2, AgeUnit::Day)),
            ("1 week ago", age(1, AgeUnit::Week)),
            ("3 weeks ago", age(3, AgeUnit::Week)),
            ("a month ago", age(1, AgeUnit::Month)),
            ("4 months ago", age(4, AgeUnit::Month)),
            ("1 year ago", age(1, AgeUnit::Year)),
            ("3 years ago", age(3, AgeUnit::Year)),
            ("soon", None),
            ("", None),
        ] {
            assert_eq!(parse_age(s), want, "{s:?}");
        }
        assert!(
            Age {
                value: 2,
                unit: AgeUnit::Week
            }
            .approx_seconds()
                < Age {
                    value: 1,
                    unit: AgeUnit::Month
                }
                .approx_seconds()
        );
    }

    #[test]
    fn adversarial_input_does_not_panic() {
        for s in [
            "<a class=\"content-block2",
            "<a class=\"content-block2 href=\"/comm-link/x/-1\"",
            "<a class=\"content-block2\" href=\"/comm-link/a/9999999999999999999999-x\">",
            "<a class=\"content-block2\" href=\"/1\"><div class=\"title é>",
        ] {
            let _ = parse_page(&json!({"success": 1, "data": s}), PROD_ROOT);
        }
    }

    /// Page 1, live.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_first_page() {
        let p = CommLinks::new("sc-dossier-test")
            .page(&CommLinkQuery::default())
            .await
            .unwrap();
        assert_eq!(p.items.len(), 10);
        assert!(p
            .items
            .iter()
            .all(|c| c.id > 0 && !c.title.is_empty() && c.age.is_some()));
    }
}
