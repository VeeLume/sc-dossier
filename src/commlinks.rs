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
//! The listing carries no video ids. [`CommLinks::videos`] reads them from the
//! comm-link's own page: two GETs (the page, then its Alexandria fragment;
//! older pages, up to id ~18160, have the embed inline: one GET).
//!
//! No pacing, caching, or retries — the caller's policy. The listing's rate
//! limit is unmeasured; stay under ~1 request/second.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::html::{alexandria_url, between, decode_entities, squash, text, tokens, Tok};

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

    /// The videos a comm-link embeds, in page order — a [`CommLink::url`] or a
    /// site path. Every sampled [`CommLinkKind::Video`] comm-link (2013 → now)
    /// has exactly one; posts usually have none (a plain YouTube *link* in the
    /// text is not counted).
    pub async fn videos(&self, url: &str) -> Result<Vec<Video>> {
        let url = if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!("{}/{}", self.root, url.trim_start_matches('/'))
        };
        let shell = self.get(&url).await?;
        match alexandria_url(&shell) {
            Some(body) => Ok(parse_videos(&self.get(&body).await?)),
            None => Ok(parse_videos(&shell)),
        }
    }

    async fn get(&self, url: &str) -> Result<String> {
        let resp = self
            .http
            .get(url)
            .header("User-Agent", &self.user_agent)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            return Err(page_err(url, &format!("HTTP {status}")));
        }
        Ok(resp.text().await?)
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

/// An embedded video.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Video {
    /// Where it's hosted.
    pub host: VideoHost,
    /// The host's id: YouTube `zNL_FgoTrjA`, Vimeo `58209081`.
    pub id: String,
}

/// Video host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VideoHost {
    /// YouTube (all comm-links since 2013).
    YouTube,
    /// Vimeo (a few 2013 comm-links).
    Vimeo,
}

impl Video {
    /// The video's own page: `https://www.youtube.com/watch?v=<id>` or
    /// `https://vimeo.com/<id>`.
    pub fn watch_url(&self) -> String {
        match self.host {
            VideoHost::YouTube => format!("https://www.youtube.com/watch?v={}", self.id),
            VideoHost::Vimeo => format!("https://vimeo.com/{}", self.id),
        }
    }
}

/// Embeds in page order, deduplicated. Three markups, by comm-link id:
/// `<g-trailer video-id="…">` (~19950 →), `<iframe src=".../embed/<id>">`
/// inside `<g-trailer>` (~18170–19060), and a server-rendered
/// `<iframe data-src="//www.youtube.com/embed/<id>">` (older). The page's
/// `{$video_id}` modal templates sit in `<script>`, which the tokenizer skips.
fn parse_videos(html: &str) -> Vec<Video> {
    let mut out: Vec<Video> = Vec::new();
    for tok in tokens(html) {
        let Tok::Start { name, attrs } = tok else {
            continue;
        };
        let found = attrs
            .iter()
            .find_map(|(k, v)| match (name.as_str(), k.as_str()) {
                ("g-trailer", "video-id") => youtube_id(v).map(|id| Video {
                    host: VideoHost::YouTube,
                    id: id.to_string(),
                }),
                ("iframe", "src" | "data-src") => embed(v),
                _ => None,
            });
        if let Some(v) = found.filter(|v| !out.contains(v)) {
            out.push(v);
        }
    }
    out
}

/// `…youtube.com/embed/<id>?…`, `…youtube-nocookie.com/embed/<id>`,
/// `…vimeo.com/video/<id>`.
fn embed(src: &str) -> Option<Video> {
    let tail = |marker: &str| src.split_once(marker).map(|(_, t)| t);
    if let Some(t) = tail("youtube.com/embed/").or_else(|| tail("youtube-nocookie.com/embed/")) {
        let id = t.split(['?', '&', '/', '#']).next()?;
        return youtube_id(id).map(|id| Video {
            host: VideoHost::YouTube,
            id: id.to_string(),
        });
    }
    let id = tail("vimeo.com/video/")?
        .split(['?', '&', '/', '#'])
        .next()?;
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())).then(|| Video {
        host: VideoHost::Vimeo,
        id: id.to_string(),
    })
}

/// YouTube ids are 11 chars of `[A-Za-z0-9_-]`.
fn youtube_id(s: &str) -> Option<&str> {
    let s = s.trim();
    (s.len() == 11
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-'))
    .then_some(s)
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

    const VIDEO_CURRENT: &str = include_str!("../tests/fixtures/commlinks/video_21381.body.html");
    const VIDEO_NOCOOKIE: &str = include_str!("../tests/fixtures/commlinks/video_19053.body.html");
    const VIDEO_IFRAME: &str = include_str!("../tests/fixtures/commlinks/video_18172.body.html");
    const VIDEO_LEGACY: &str = include_str!("../tests/fixtures/commlinks/video_12853.shell.html");

    fn yt(id: &str) -> Video {
        Video {
            host: VideoHost::YouTube,
            id: id.into(),
        }
    }

    #[test]
    fn videos_in_every_markup() {
        assert_eq!(parse_videos(VIDEO_CURRENT), [yt("zNL_FgoTrjA")]);
        assert_eq!(parse_videos(VIDEO_NOCOOKIE), [yt("v3JNtoMi_a4")]);
        assert_eq!(parse_videos(VIDEO_IFRAME), [yt("-OaMjK9nfzE")]);
        // The legacy page also holds the `{$video_id}` modal templates.
        assert!(VIDEO_LEGACY.contains("youtube.com/embed/{$video_id}"));
        assert_eq!(parse_videos(VIDEO_LEGACY), [yt("J46tDrM-jHo")]);
        // 2013 Vimeo embed (comm-link 12867), verbatim.
        let vimeo = r#"<p><iframe src="http://player.vimeo.com/video/58209081" width="400" height="300" frameborder="0"></iframe></p>"#;
        let v = parse_videos(vimeo);
        assert_eq!(
            v,
            [Video {
                host: VideoHost::Vimeo,
                id: "58209081".into()
            }]
        );
        assert_eq!(v[0].watch_url(), "https://vimeo.com/58209081");
        assert_eq!(
            yt("zNL_FgoTrjA").watch_url(),
            "https://www.youtube.com/watch?v=zNL_FgoTrjA"
        );
    }

    #[test]
    fn videos_ignore_links_templates_and_duplicates() {
        let html = r#"<a href="https://www.youtube.com/watch?v=i4rVtLAenmk">TWISC</a>
            <iframe src="https://www.youtube.com/embed/{$video_id}?autoplay=1"></iframe>
            <g-trailer video-id="not-an-id"></g-trailer>
            <g-trailer video-id="zNL_FgoTrjA"></g-trailer>
            <iframe src="https://www.youtube.com/embed/zNL_FgoTrjA?enablejsapi=1"></iframe>"#;
        assert_eq!(parse_videos(html), [yt("zNL_FgoTrjA")]);
        assert!(parse_videos(PAGE1).is_empty());
        for s in [
            "<iframe src=\"vimeo.com/video/\">",
            "<g-trailer video-id=\"é\">",
            "<iframe data-src=\"youtube.com/embed/",
        ] {
            assert!(parse_videos(s).is_empty(), "{s:?}");
        }
    }

    /// The newest video comm-link's video, live.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_video() {
        let cl = CommLinks::new("sc-dossier-test");
        let q = CommLinkQuery {
            kind: Some(CommLinkKind::Video),
            ..Default::default()
        };
        let page = cl.page(&q).await.unwrap();
        let videos = cl.videos(&page.items[0].url).await.unwrap();
        assert_eq!(videos.len(), 1, "{videos:?}");
        assert_eq!(videos[0].host, VideoHost::YouTube);
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
