//! Hotfix Central: the pinned thread in SC's Patch Notes forum whose opening
//! post CIG edits with every LIVE hotfix.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{id, Spectrum};
use crate::error::{Error, Result};

/// SC's Patch Notes forum channel.
const PATCH_NOTES_CHANNEL: &str = "190048";

impl Spectrum {
    /// The current Hotfix Central thread, parsed: the newest pinned thread in
    /// the Patch Notes forum titled "Hotfix Central" (one per major version).
    /// `None` when none is pinned. Two calls (v1 `/forum/channel/threads`,
    /// v1 `/forum/thread/reply`); works anonymously.
    pub async fn hotfix_central(&self) -> Result<Option<HotfixCentral>> {
        let body = json!({
            "channel_id": PATCH_NOTES_CHANNEL,
            "page": 1,
            "sort": "newest",
            "label_id": null,
            // Pinned threads come on top of the page size.
            "pagesize": 1,
        });
        let data = self.call("/forum/channel/threads", Some(&body)).await?;
        let Some(thread) = current_thread(&data) else {
            return Ok(None);
        };
        let reply_id = id(thread, "/content_reply_id").ok_or_else(|| {
            Error::Invalid("Hotfix Central thread has no content_reply_id".into())
        })?;
        let opener = self
            .call(
                "/forum/thread/reply",
                Some(&json!({ "reply_id": reply_id })),
            )
            .await?;
        HotfixCentral::from_values(thread, &opener, &self.root)
            .map(Some)
            .ok_or_else(|| {
                Error::Invalid(format!("Hotfix Central reply {reply_id}: unexpected shape"))
            })
    }
}

/// The parsed Hotfix Central thread from [`Spectrum::hotfix_central`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotfixCentral {
    /// Thread id.
    pub thread_id: String,
    /// The opening post's reply id.
    pub reply_id: String,
    /// URL slug (not derivable from the version: `…-4-10-live-…`, `…-4-5-0-…`).
    pub slug: String,
    /// Title, e.g. "Star Citizen Alpha 4.10.2 LIVE - Hotfix Central (Updated 10.14.2026)".
    pub subject: String,
    /// Web URL.
    pub url: String,
    /// Thread creation, Unix milliseconds.
    pub time_ms: i64,
    /// Last edit of the opening post, Unix milliseconds — changes with each
    /// update. (The thread's own modified time follows replies instead.)
    pub edited_ms: i64,
    /// The "Current … LIVE Status" block.
    pub status: HotfixStatus,
    /// Hotfix entries, newest first, as posted.
    pub entries: Vec<HotfixEntry>,
    /// The whole opening post as plain text, one line per block — for when
    /// the parse misses something.
    pub raw: String,
}

/// The status block at the top of Hotfix Central.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotfixStatus {
    /// Heading, e.g. "Current 4.10.2 LIVE Status | 10.14.2026".
    pub heading: String,
    /// LIVE version from the heading, e.g. `"4.10.2"`.
    pub version: Option<String>,
    /// Date from the heading.
    pub date: Option<NaiveDate>,
    /// "Status: Hotfix Channel Currently OPEN|CLOSED" → `true`|`false`.
    pub hotfix_channel_open: Option<bool>,
    /// "Latest LIVE Client CL: N".
    pub live_client_cl: Option<u64>,
    /// "Latest LIVE DGS CL: N (…)" — the first number; the rest is in `lines`.
    pub live_dgs_cl: Option<u64>,
    /// Every non-empty line of the block, as posted (links included).
    pub lines: Vec<String>,
}

/// One hotfix entry: a "► M.D.YYYY: headline" line and what follows it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HotfixEntry {
    /// Date (US order, as posted).
    pub date: Option<NaiveDate>,
    /// Text after the date, e.g. "Hotfix 12913470 - Now on the LIVE channel";
    /// may be empty.
    pub headline: String,
    /// Changelists (7–9 digit numbers) in the headline, in order.
    pub builds: Vec<u64>,
    /// Channel the headline puts the build on — `"HOTFIX"` or `"LIVE"`, the
    /// first named ("on the HOTFIX channel", "now on LIVE"); `None` for
    /// server-only canaries and the like.
    pub channel: Option<String>,
    /// The lines below the headline.
    pub items: Vec<HotfixItem>,
}

/// A line of a [`HotfixEntry`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HotfixItem {
    /// Text.
    pub text: String,
    /// A bullet-list item (else a paragraph, e.g. "Server Side" or a summary).
    pub bullet: bool,
}

/// Newest pinned "Hotfix Central" thread in a v1 `/forum/channel/threads` page.
fn current_thread(data: &Value) -> Option<&Value> {
    data.get("threads")?
        .as_array()?
        .iter()
        .filter(|t| t.get("is_pinned").and_then(Value::as_bool) == Some(true))
        .filter(|t| {
            t.get("subject")
                .and_then(Value::as_str)
                .is_some_and(|s| s.to_ascii_lowercase().contains("hotfix central"))
        })
        .max_by_key(|t| secs(t, "/time_created"))
}

impl HotfixCentral {
    fn from_values(thread: &Value, opener: &Value, root: &str) -> Option<Self> {
        let blocks = draft_blocks(opener);
        let slug = thread.get("slug")?.as_str()?.to_string();
        let (status, entries) = parse_blocks(&blocks);
        Some(Self {
            thread_id: id(thread, "/id")?,
            reply_id: id(opener, "/id")?,
            url: format!("{root}/spectrum/community/SC/forum/{PATCH_NOTES_CHANNEL}/thread/{slug}"),
            slug,
            subject: thread
                .get("subject")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            time_ms: secs(thread, "/time_created") * 1000,
            edited_ms: secs(opener, "/time_modified") * 1000,
            status,
            entries,
            raw: blocks
                .iter()
                .map(|(_, t)| t.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        })
    }
}

/// v1 times are Unix seconds, as a string or a number.
fn secs(v: &Value, pointer: &str) -> i64 {
    id(v, pointer).and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// `(block type, text)` of every Draft.js block in a v1 reply's `content_blocks`.
fn draft_blocks(reply: &Value) -> Vec<(String, String)> {
    reply
        .get("content_blocks")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|c| c.get("type").and_then(Value::as_str) == Some("text"))
        .filter_map(|c| c.pointer("/data/blocks")?.as_array())
        .flatten()
        .map(|b| {
            let s = |k: &str| b.get(k).and_then(Value::as_str).unwrap_or("").to_string();
            (s("type"), s("text"))
        })
        .collect()
}

/// Intro, then a `header-one` "Current … Status" block, then `►` entries,
/// then a closing `header-one` ("Using the HOTFIX channel"). The first entry or
/// the next heading ends the status block; a heading after it ends the entries.
fn parse_blocks(blocks: &[(String, String)]) -> (HotfixStatus, Vec<HotfixEntry>) {
    #[derive(PartialEq)]
    enum Part {
        Intro,
        Status,
        Entries,
        Done,
    }
    let mut part = Part::Intro;
    let mut status = HotfixStatus {
        heading: String::new(),
        version: None,
        date: None,
        hotfix_channel_open: None,
        live_client_cl: None,
        live_dgs_cl: None,
        lines: Vec::new(),
    };
    let mut entries: Vec<HotfixEntry> = Vec::new();
    for (kind, text) in blocks {
        let text = text.trim();
        if text.is_empty() || part == Part::Done {
            continue;
        }
        if kind == "header-one" {
            if part == Part::Intro && text.to_ascii_lowercase().starts_with("current") {
                part = Part::Status;
                status.heading = text.to_string();
                let (head, date) = text.split_once('|').unwrap_or((text, ""));
                status.version = head
                    .split_whitespace()
                    .find(|w| w.starts_with(|c: char| c.is_ascii_digit()) && w.contains('.'))
                    .map(str::to_string);
                status.date = us_date(date.trim());
            } else if part != Part::Intro {
                part = Part::Done;
            }
            continue;
        }
        if let Some(rest) = text.strip_prefix('►') {
            part = Part::Entries;
            entries.push(entry(rest.trim()));
            continue;
        }
        match part {
            Part::Status => status_line(&mut status, text),
            Part::Entries => {
                if let Some(e) = entries.last_mut() {
                    e.items.push(HotfixItem {
                        text: text.to_string(),
                        bullet: kind.ends_with("list-item"),
                    });
                }
            }
            Part::Intro | Part::Done => {}
        }
    }
    (status, entries)
}

fn status_line(status: &mut HotfixStatus, line: &str) {
    let lower = line.to_ascii_lowercase();
    let after_colon = line.split_once(':').map_or("", |(_, r)| r);
    if lower.starts_with("status:") && lower.contains("hotfix") {
        status.hotfix_channel_open = if lower.contains("closed") {
            Some(false)
        } else if lower.contains("open") {
            Some(true)
        } else {
            None
        };
    } else if lower.contains("client cl") {
        status.live_client_cl = changelists(after_colon).first().copied();
    } else if lower.contains("dgs cl") {
        status.live_dgs_cl = changelists(after_colon).first().copied();
    }
    status.lines.push(line.to_string());
}

/// "10.13.2026: Hotfix 12913470 - Now on the LIVE channel".
fn entry(text: &str) -> HotfixEntry {
    let end = text
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .unwrap_or(text.len());
    let date = us_date(&text[..end]);
    let headline = if date.is_some() {
        text[end..].trim_start_matches([':', '-', ' ']).trim()
    } else {
        text
    };
    HotfixEntry {
        date,
        headline: headline.to_string(),
        builds: changelists(headline),
        channel: channel(headline),
        items: Vec::new(),
    }
}

/// "10.14.2026" or "1.8.26" (month first).
fn us_date(s: &str) -> Option<NaiveDate> {
    let mut parts = s.trim_end_matches('.').split('.');
    let m = parts.next()?.parse().ok()?;
    let d = parts.next()?.parse().ok()?;
    let y: i32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let y = if y < 100 { 2000 + y } else { y };
    NaiveDate::from_ymd_opt(y, m, d)
}

/// 7–9 digit runs (changelists), deduplicated, in order.
fn changelists(s: &str) -> Vec<u64> {
    let mut out = Vec::new();
    for run in s.split(|c: char| !c.is_ascii_digit()) {
        if (7..=9).contains(&run.len()) {
            if let Ok(n) = run.parse() {
                if !out.contains(&n) {
                    out.push(n);
                }
            }
        }
    }
    out
}

/// The first channel a headline puts the build on.
fn channel(headline: &str) -> Option<String> {
    let h = headline.to_ascii_lowercase();
    [
        ("hotfix channel", "HOTFIX"),
        ("live channel", "LIVE"),
        ("on the live", "LIVE"),
        ("on live", "LIVE"),
        ("to live", "LIVE"),
    ]
    .iter()
    .filter_map(|(pat, ch)| h.find(pat).map(|i| (i, *ch)))
    .min_by_key(|(i, _)| *i)
    .map(|(_, ch)| ch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PINNED: &str = include_str!("../../tests/fixtures/spectrum/hotfix_pinned.json");
    const OPENER_4_10: &str = include_str!("../../tests/fixtures/spectrum/hotfix_opener_4_10.json");
    const OPENER_4_7: &str = include_str!("../../tests/fixtures/spectrum/hotfix_opener_4_7.json");
    const OPENER_4_2: &str = include_str!("../../tests/fixtures/spectrum/hotfix_opener_4_2.json");

    fn data(raw: &str) -> Value {
        serde_json::from_str::<Value>(raw).unwrap()["data"].clone()
    }

    fn central(opener: &str) -> HotfixCentral {
        let threads = data(PINNED);
        let t = current_thread(&threads).unwrap();
        HotfixCentral::from_values(t, &data(opener), super::super::PROD_ROOT).unwrap()
    }

    fn date(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d)
    }

    #[test]
    fn finds_the_pinned_thread() {
        let threads = data(PINNED);
        let t = current_thread(&threads).unwrap();
        assert_eq!(t["slug"], "star-citizen-alpha-4-10-live-hotfix-central");
        assert!(current_thread(&json!({"threads": []})).is_none());
        assert!(current_thread(
            &json!({"threads": [{"is_pinned": false, "subject": "Hotfix Central"}]})
        )
        .is_none());
    }

    #[test]
    fn parses_4_10() {
        let h = central(OPENER_4_10);
        assert_eq!(h.thread_id, "568266");
        assert_eq!(h.reply_id, "9103415");
        assert_eq!(
            h.url,
            "https://robertsspaceindustries.com/spectrum/community/SC/forum/190048/thread/star-citizen-alpha-4-10-live-hotfix-central"
        );
        assert_eq!(h.time_ms, 1_787_845_815_000);
        assert_eq!(h.edited_ms, 1_792_012_586_000);
        let s = &h.status;
        assert_eq!(s.heading, "Current 4.10.2 LIVE Status | 10.14.2026");
        assert_eq!(s.version.as_deref(), Some("4.10.2"));
        assert_eq!(s.date, date(2026, 10, 14));
        assert_eq!(s.hotfix_channel_open, Some(false));
        assert_eq!(
            (s.live_client_cl, s.live_dgs_cl),
            (Some(12913470), Some(12913470))
        );
        assert!(s.lines.iter().any(|l| l.contains("12939222 testing")));

        assert_eq!(h.entries.len(), 6);
        let e = &h.entries[0];
        assert_eq!(e.date, date(2026, 10, 14));
        assert_eq!(
            e.headline,
            "DGS Hotfix 12939222 - Canaried onto multiple shards for testing"
        );
        assert_eq!(
            (e.builds.as_slice(), e.channel.as_deref()),
            (&[12939222][..], None)
        );
        assert_eq!(e.items.len(), 1);
        let e = &h.entries[1];
        assert_eq!(
            (e.builds.as_slice(), e.channel.as_deref()),
            (&[12913470][..], Some("LIVE"))
        );
        assert_eq!(e.items.len(), 2);
        assert!(e.items.iter().all(|i| i.bullet));
        let e = &h.entries[3];
        assert_eq!(e.date, date(2026, 9, 2));
        assert_eq!(e.items.len(), 5);
        assert!(!e.items[0].bullet && e.items[1].bullet);
        let e = &h.entries[5];
        assert_eq!(e.channel.as_deref(), Some("HOTFIX"), "{}", e.headline);
        // The trailer is not part of the last entry.
        assert!(e
            .items
            .iter()
            .all(|i| !i.text.contains("rename your LIVE folder")));
        assert!(h.raw.contains("Using the HOTFIX channel"));
        let dates: Vec<_> = h.entries.iter().map(|e| e.date).collect();
        assert!(dates.windows(2).all(|w| w[0] >= w[1]), "newest first");
    }

    #[test]
    fn parses_older_layouts() {
        let h = central(OPENER_4_7);
        assert_eq!(h.status.version.as_deref(), Some("4.7.2"));
        assert_eq!(h.status.live_client_cl, Some(11715810));
        assert!(h.entries.len() >= 10);
        assert!(h.entries.iter().all(|e| e.date.is_some()));
        let bare = h
            .entries
            .iter()
            .find(|e| e.date == date(2026, 3, 26))
            .unwrap();
        assert_eq!(bare.headline, "");
        assert_eq!(bare.items.len(), 3);
        let glued = h
            .entries
            .iter()
            .find(|e| e.headline.contains("11674325is"))
            .unwrap();
        assert_eq!(glued.builds, [11674325]);

        // 4.2.1: `►` entries as plain paragraphs, no status bullets, 7-digit CLs.
        let h = central(OPENER_4_2);
        assert_eq!(h.status.version.as_deref(), Some("4.2.1"));
        assert_eq!(h.status.hotfix_channel_open, None);
        assert!(h.entries.len() >= 3);
        assert_eq!(h.entries[0].date, date(2025, 7, 29));
        assert!(h.entries.iter().any(|e| e.builds.contains(&9982497)));
    }

    #[test]
    fn small_parsers() {
        assert_eq!(us_date("1.8.26"), date(2026, 1, 8));
        assert_eq!(us_date("13.1.2026"), None);
        assert_eq!(us_date("7.1"), None);
        assert_eq!(
            changelists("12545750_010 and 2026 and 1234567890"),
            [12545750]
        );
        for (h, want) in [
            (
                "Hotfix 12535871  is now on the HOTFIX channel and select shards on LIVE",
                Some("HOTFIX"),
            ),
            (
                "Client/DGS Hotfix channel open and DGS canaried - 12286454",
                Some("HOTFIX"),
            ),
            (
                "Client Hotfix 12572603 - Client side now on LIVE",
                Some("LIVE"),
            ),
            ("4.7.1 is now on the LIVE environment!", Some("LIVE")),
            ("Server Side Canary - 12258450", None),
        ] {
            assert_eq!(channel(h).as_deref(), want, "{h}");
        }
        let e = entry("► 3.26.2026".trim_start_matches('►').trim());
        assert_eq!((e.date, e.headline.as_str()), (date(2026, 3, 26), ""));
        let (s, e) = parse_blocks(&[]);
        assert!(s.heading.is_empty() && e.is_empty());
    }

    /// The current Hotfix Central, live.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_hotfix_central() {
        let h = Spectrum::anonymous("sc-dossier-test")
            .hotfix_central()
            .await
            .unwrap()
            .expect("a pinned Hotfix Central");
        assert!(h.edited_ms > 0 && !h.entries.is_empty());
        assert!(h.status.version.is_some());
    }
}
