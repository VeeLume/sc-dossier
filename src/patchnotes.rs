//! LIVE patch notes from the public RSI website — anonymous, no launcher
//! session.
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! use sc_dossier::patchnotes::PatchNotes;
//! let notes = PatchNotes::new("my-app/1.0");
//! let index = notes.index().await?;
//! let article = notes.article(&index[0].url).await?;
//! for r in &article.releases {
//!     println!("{} {:?}: {}", r.title, r.date, r.highlights.join(" · "));
//! }
//! # Ok(()) }
//! ```
//!
//! [`PatchNotes::index`] reads `/en/patch-notes`: one card per article, newest
//! first. An article is the detail page's Alexandria fragment, parsed into
//! [`Release`]s. Minor-version articles can accumulate point releases (4.7,
//! 4.8: each point release is prepended under its own dated banner), so one
//! article may hold several releases, newest first.
//!
//! Text only: entities decoded, whitespace normalised, no HTML. Two article
//! templates are covered — FAQ-style features (≤ 4.10.1) and MiniGrid /
//! ArtemisNarrative (4.10.2+); template filler (lorem ipsum) and the link-card
//! boilerplate are dropped.
//!
//! No pacing, caching, or retries — the caller's policy.

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{Error, Result};
use crate::html::{between, decode_entities, squash, tokens, Tok};

/// Production host.
pub const PROD_ROOT: &str = "https://robertsspaceindustries.com";

/// Anonymous client for the patch-note pages.
#[derive(Debug, Clone)]
pub struct PatchNotes {
    http: reqwest::Client,
    root: String,
    user_agent: String,
}

impl PatchNotes {
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

    /// Every patch-note article, newest first (`GET /en/patch-notes`).
    pub async fn index(&self) -> Result<Vec<PatchNoteRef>> {
        let url = format!("{}/en/patch-notes", self.root);
        let html = self.get(&url).await?;
        let refs = parse_index(&html, &self.root);
        if refs.is_empty() {
            return Err(page(&url, "no patch-note cards on the index page"));
        }
        Ok(refs)
    }

    /// One article by URL — a [`PatchNoteRef::url`], or a site path like
    /// `/comm-link/Patch-Notes/21351-Star-Citizen-Alpha-4102`. Fetches the
    /// detail page, then the Alexandria fragment it loads.
    pub async fn article(&self, url: &str) -> Result<PatchNoteArticle> {
        let url = self.absolute(url);
        let shell = self.get(&url).await?;
        let body_url = alexandria_url(&shell)
            .ok_or_else(|| page(&url, "no Alexandria content URL (s3Url) in the page"))?;
        let body_url = self.absolute(&body_url);
        let fragment = self.get(&body_url).await?;
        let releases = parse_releases(&fragment);
        if releases.is_empty() {
            return Err(page(&body_url, "no dated release banner in the article"));
        }
        let title = og_title(&shell).unwrap_or_else(|| releases[0].title.clone());
        Ok(PatchNoteArticle {
            id: article_id(&url).unwrap_or(0),
            url,
            title,
            releases,
        })
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
            return Err(page(url, &format!("HTTP {status}")));
        }
        Ok(resp.text().await?)
    }

    fn absolute(&self, url: &str) -> String {
        if url.starts_with("http://") || url.starts_with("https://") {
            url.to_string()
        } else {
            format!("{}/{}", self.root, url.trim_start_matches('/'))
        }
    }
}

fn page(url: &str, what: &str) -> Error {
    Error::Page {
        url: url.to_string(),
        what: what.to_string(),
    }
}

/// An index entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchNoteRef {
    /// Comm-link id (`21351`).
    pub id: u64,
    /// Absolute article URL.
    pub url: String,
    /// Card title, e.g. "Star Citizen Alpha 4.10.2".
    pub title: String,
    /// Version from the title (`"4.10.2"`, `"4.10"`).
    pub version: Option<String>,
    /// ~35 KB `heap_infobox` variant of the card image.
    pub thumbnail: Option<String>,
    /// **Relative** age as the index shows it ("6 days ago", "1 year ago") —
    /// the index carries no absolute date; [`Release::date`] does.
    pub age_text: String,
}

/// One patch-note article.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PatchNoteArticle {
    /// Comm-link id.
    pub id: u64,
    /// Absolute article URL.
    pub url: String,
    /// Page title, e.g. "Star Citizen Alpha 4.8".
    pub title: String,
    /// Newest first; several for articles that accumulate point releases.
    pub releases: Vec<Release>,
}

/// One LIVE release.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Release {
    /// The banner date as written ("October 9th, 2026", "June 19, 2025").
    pub date_text: String,
    /// [`Release::date_text`] parsed; `None` if it doesn't parse.
    pub date: Option<NaiveDate>,
    /// "Star Citizen Alpha 4.10.2 LIVE".
    pub title: String,
    /// Version as the title names it (`"4.10.2"`; `"4.10"` where the build
    /// says `4.10.0`), falling back to the build's.
    pub version: Option<String>,
    /// `"LIVE"` — from the build line, else the title.
    pub channel: Option<String>,
    /// The patch name ("RSI Discovery Month"; "Update" on point releases).
    pub name: Option<String>,
    /// From the "VERSION …" line.
    pub build: Option<Build>,
    /// Build-info items as `(label, value)`: "Starting aUEC" → "20,000".
    /// Colon-less items (3.24.3's "Items not saved to LTP" list) take the
    /// preceding paragraph as their label.
    pub build_info: Vec<(String, String)>,
    /// One-line headline items: the features' FAQ titles (≤ 4.10.1, already
    /// summaries) or their h1–h3 titles (4.10.2+), never "Additional …".
    /// Falls back to the first section for point releases without a features
    /// section. May be empty.
    pub highlights: Vec<String>,
    /// List-item counts for an overview.
    pub counts: Counts,
    /// Content sections in page order.
    pub sections: Vec<Section>,
}

/// List items (`<li>`, nested sub-items not counted separately) per section
/// kind. `None` when the release has no such section or it holds no list —
/// e.g. newer Known Issues only link the support KB, and point releases put
/// their fixes in an "Updates" FAQ entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Counts {
    /// [`SectionKind::BugFixes`] items (4.10.2+ includes its "Hotfixes
    /// Already Applied to Live").
    pub bug_fixes: Option<u32>,
    /// [`SectionKind::KnownIssues`] items.
    pub known_issues: Option<u32>,
}

/// The `VERSION` line.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Build {
    /// As published: `"4.10.2-LIVE.12881860"` (4.8.0's 7-digit
    /// `"4.8.0-LIVE.1825000"` is kept as is).
    pub raw: String,
    /// The trailing build number (`"12881860"`).
    pub changelist: Option<String>,
}

/// A section under an undated banner.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Section {
    /// "Features and Gameplay", "Bug Fixes & Technical Updates", … — or
    /// "Updates" for content a point release puts straight under its banner.
    pub title: String,
    /// Classified from the title.
    pub kind: SectionKind,
    /// Content in page order.
    pub blocks: Vec<Block>,
}

/// What a [`Section`] holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SectionKind {
    /// "Features and Gameplay".
    Features,
    /// "Bug Fixes & Technical Updates".
    BugFixes,
    /// "Known Issues & Information".
    KnownIssues,
    /// Anything else (point-release "Updates").
    Other,
}

/// Text content.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Block {
    /// A heading; bold-only short paragraphs count as one level below the
    /// heading before them (at least 4).
    Heading {
        /// 1–6.
        level: u8,
        /// Heading text.
        text: String,
    },
    /// A paragraph; `lead` is a bold lead-in such as "Bridge:".
    Paragraph {
        /// Bold lead-in, if any.
        lead: Option<String>,
        /// The rest of the paragraph.
        text: String,
    },
    /// List items. Nested sub-items (fix explanations) are appended to their
    /// parent item, one per line.
    List(Vec<String>),
    /// A titled FAQ entry (older feature write-ups, point-release updates).
    Entry {
        /// Entry title — a one-line summary.
        title: String,
        /// Entry content.
        blocks: Vec<Block>,
    },
    /// A paragraph that is only a link (e.g. the support KB).
    Link {
        /// Link text.
        text: String,
        /// Absolute URL.
        url: String,
    },
}

// ---------------------------------------------------------------------------
// Index

fn parse_index(html: &str, root: &str) -> Vec<PatchNoteRef> {
    html.split("<a class=\"content-block2")
        .skip(1)
        .filter_map(|card| {
            let href = between(card, "href=\"", "\"")?;
            let url = if href.starts_with('/') {
                format!("{root}{href}")
            } else {
                href.to_string()
            };
            let id = article_id(&url)?;
            let title = between(card, "class=\"title ", "</div>")
                .and_then(|t| t.split_once('>'))
                .map(|(_, t)| squash(&decode_entities(t)))
                .unwrap_or_default();
            let thumbnail = between(card, "background-image:url('", "'")
                .map(|u| u.replace("/channel_item_full.", "/heap_infobox."));
            Some(PatchNoteRef {
                id,
                url,
                version: title_version(&title),
                title,
                thumbnail,
                age_text: between(card, "class=\"value\">", "<")
                    .map(squash)
                    .unwrap_or_default(),
            })
        })
        .collect()
}

/// `…/Patch-Notes/21351-Star-Citizen-Alpha-4102` → 21351.
fn article_id(url: &str) -> Option<u64> {
    let last = url.trim_end_matches('/').rsplit('/').next()?;
    last.split('-').next()?.parse().ok()
}

/// `const s3Url = '<url>';` in the detail page.
fn alexandria_url(shell: &str) -> Option<String> {
    let decl = between(shell, "const s3Url", ";")?;
    let q = decl.find(['\'', '"'])?;
    let quote = &decl[q..q + 1];
    between(&decl[q..], quote, quote).map(str::to_string)
}

fn og_title(shell: &str) -> Option<String> {
    let t = between(shell, "<meta property=\"og:title\" content=\"", "\"")?;
    let t = squash(&decode_entities(t));
    (!t.is_empty()).then_some(t)
}

// ---------------------------------------------------------------------------
// Article fragment → releases

/// Section title for content a point release puts straight under its banner.
const IMPLICIT_SECTION: &str = "Updates";

/// Walk the fragment's components in order: a dated `g-banner-advanced` opens
/// a release, an undated one a section. Content before a release's first
/// section is its intro (only the VERSION line is kept), except FAQ entries,
/// which open the implicit section. Components that don't parse are skipped.
fn parse_releases(fragment: &str) -> Vec<Release> {
    let mut out: Vec<Release> = Vec::new();
    for tok in tokens(fragment) {
        let Tok::Start { name, attrs } = tok else {
            continue;
        };
        let attr = |k: &str| attrs.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        let json = |k: &str| attr(k).and_then(|raw| serde_json::from_str::<Value>(raw).ok());
        match name.as_str() {
            "g-banner-advanced" => {
                let Some(v) = json(":content") else { continue };
                let text = |k: &str| {
                    v.pointer(&format!("/text/{k}"))
                        .and_then(Value::as_str)
                        .map(plain_text)
                        .unwrap_or_default()
                };
                let date_text = text("overline");
                if !date_text.is_empty() {
                    out.push(new_release(date_text, text("title"), text("subtitle")));
                } else if let Some(r) = out.last_mut() {
                    let title = text("title");
                    r.sections.push(Section {
                        kind: section_kind(&title),
                        title,
                        blocks: Vec::new(),
                    });
                }
            }
            "g-introduction" => {
                let (Some(v), Some(r)) = (json(":info"), out.last_mut()) else {
                    continue;
                };
                for c in v
                    .get("contents")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    r.build_info
                        .extend(build_info(c.as_str().unwrap_or_default()));
                }
            }
            "g-article" => {
                if let (Some(body), Some(r)) = (attr("body"), out.last_mut()) {
                    content(r, None, body);
                }
            }
            "g-faq" => {
                let (Some(v), Some(r)) = (json(":question-list"), out.last_mut()) else {
                    continue;
                };
                for q in v.as_array().into_iter().flatten() {
                    let title = q.get("title").and_then(Value::as_str).map(plain_text);
                    let body = q.get("content").and_then(Value::as_str).unwrap_or_default();
                    content(r, Some(title.unwrap_or_default()), body);
                }
            }
            "g-platform-client-component" => {
                let (Some(v), Some(r)) = (json(":properties"), out.last_mut()) else {
                    continue;
                };
                match v.get("componentId").and_then(Value::as_str) {
                    Some("MiniGrid") => {
                        let els = v
                            .pointer("/componentProps/gridOptions/uiData/elements")
                            .and_then(Value::as_array);
                        for e in els.into_iter().flatten() {
                            if e.get("key").and_then(Value::as_str) == Some("mg.text") {
                                let t = e.pointer("/data/text").and_then(Value::as_str);
                                content(r, None, t.unwrap_or_default());
                            }
                        }
                    }
                    Some("ArtemisNarrative") => {
                        // `value.body` is the article; `value.value.body` is
                        // the template's lorem-ipsum default.
                        let blocks = v
                            .pointer("/componentProps/blocks")
                            .and_then(Value::as_array);
                        for b in blocks.into_iter().flatten() {
                            if let Some(t) = b.pointer("/value/body").and_then(Value::as_str) {
                                content(r, None, t);
                            }
                        }
                    }
                    _ => {}
                }
            }
            // g-grid (link cards with lorem descriptions), g-navigation-sales,
            // rawmarkup anchors: boilerplate.
            _ => {}
        }
    }
    for r in &mut out {
        r.sections.retain(|s| !s.blocks.is_empty());
        r.highlights = highlights(&r.sections);
        r.counts = Counts {
            bug_fixes: count_items(&r.sections, SectionKind::BugFixes),
            known_issues: count_items(&r.sections, SectionKind::KnownIssues),
        };
    }
    out
}

fn count_items(sections: &[Section], kind: SectionKind) -> Option<u32> {
    fn count(blocks: &[Block]) -> usize {
        blocks
            .iter()
            .map(|b| match b {
                Block::List(items) => items.len(),
                Block::Entry { blocks, .. } => count(blocks),
                _ => 0,
            })
            .sum()
    }
    let n: usize = sections
        .iter()
        .filter(|s| s.kind == kind)
        .map(|s| count(&s.blocks))
        .sum();
    (n > 0).then(|| u32::try_from(n).unwrap_or(u32::MAX))
}

fn new_release(date_text: String, title: String, name: String) -> Release {
    Release {
        date: parse_date(&date_text),
        date_text,
        version: title_version(&title),
        channel: title_channel(&title),
        name: (!name.is_empty()).then_some(name),
        title,
        build: None,
        build_info: Vec::new(),
        highlights: Vec::new(),
        counts: Counts::default(),
        sections: Vec::new(),
    }
}

/// Route one content component of the current release; `faq_title` is `Some`
/// for a FAQ entry.
fn content(r: &mut Release, faq_title: Option<String>, html: &str) {
    if r.sections.is_empty() {
        if r.build.is_none() {
            if let Some(b) = find_build(&inline_text(html)) {
                if r.version.is_none() {
                    r.version = Some(b.raw.split('-').next().unwrap_or(&b.raw).to_string());
                }
                if let Some(ch) = b.raw.split(['-', '.']).find(|p| is_channel(p)) {
                    r.channel = Some(ch.to_string());
                }
                r.build = Some(b);
            }
        }
        if faq_title.is_none() {
            return; // intro blurb
        }
        r.sections.push(Section {
            title: IMPLICIT_SECTION.into(),
            kind: SectionKind::Other,
            blocks: Vec::new(),
        });
    }
    let Some(sec) = r.sections.last_mut() else {
        return;
    };
    let blocks = blocks(html);
    match faq_title {
        Some(title) if !title.is_empty() || !blocks.is_empty() => {
            sec.blocks.push(Block::Entry { title, blocks })
        }
        Some(_) => {}
        None => sec.blocks.extend(blocks),
    }
}

fn section_kind(title: &str) -> SectionKind {
    let t = title.to_lowercase();
    if t.contains("known issue") {
        SectionKind::KnownIssues
    } else if t.contains("bug") {
        SectionKind::BugFixes
    } else if t.contains("feature") {
        SectionKind::Features
    } else {
        SectionKind::Other
    }
}

fn highlights(sections: &[Section]) -> Vec<String> {
    let Some(sec) = sections
        .iter()
        .find(|s| s.kind == SectionKind::Features)
        .or(sections.first())
    else {
        return Vec::new();
    };
    let mut out: Vec<String> = sec
        .blocks
        .iter()
        .filter_map(|b| match b {
            Block::Entry { title, .. } => Some(title.clone()),
            _ => None,
        })
        .collect();
    if out.is_empty() {
        out = sec
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Heading { level, text } if *level <= 3 => Some(text.clone()),
                _ => None,
            })
            .filter(|t| !t.starts_with("Additional"))
            .collect();
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|t| !t.is_empty() && seen.insert(t.clone()));
    out
}

/// `Label: value` list items; colon-less items take the preceding paragraph
/// ("Items not saved to LTP Include:") as their label.
fn build_info(html: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut label = String::new();
    for b in blocks(html) {
        match b {
            Block::Paragraph { lead, text } => {
                label = lead.map_or(text.clone(), |l| format!("{l} {text}"));
                label = label.trim().trim_end_matches(':').trim().to_string();
            }
            Block::Heading { text, .. } => label = text.trim_end_matches(':').trim().to_string(),
            Block::List(items) => {
                for item in items {
                    let item = item.lines().next().unwrap_or_default();
                    match item.split_once(':') {
                        Some((k, v)) if !k.trim().is_empty() && !v.trim().is_empty() => {
                            out.push((k.trim().to_string(), v.trim().to_string()))
                        }
                        _ if !label.is_empty() => out.push((label.clone(), item.to_string())),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Version / title / date helpers

/// "Star Citizen Alpha 4.10.2 LIVE" → "4.10.2".
fn title_version(title: &str) -> Option<String> {
    title
        .split_whitespace()
        .find(|w| w.contains('.') && w.chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(|w| w.trim_end_matches('.').to_string())
}

fn title_channel(title: &str) -> Option<String> {
    title
        .split_whitespace()
        .last()
        .filter(|w| is_channel(w))
        .map(str::to_string)
}

fn is_channel(s: &str) -> bool {
    s.len() >= 2 && s.chars().all(|c| c.is_ascii_uppercase())
}

/// The build after "VERSION": `<x.y.z>[-<CHANNEL>[.<build>]]`. The text may be
/// glued to what follows (tags dropped without spacing), so match the shape.
fn find_build(text: &str) -> Option<Build> {
    let i = text.find("VERSION")?;
    let rest = text[i + "VERSION".len()..].trim_start_matches([' ', ':']);
    let span = |s: &str, f: fn(char) -> bool| s.find(|c: char| !f(c)).unwrap_or(s.len());
    let mut n = rest[..span(rest, |c| c.is_ascii_digit() || c == '.')]
        .trim_end_matches('.')
        .len();
    if n == 0 {
        return None;
    }
    let mut changelist = None;
    if let Some(ch) = rest[n..].strip_prefix('-') {
        let c = span(ch, |c| c.is_ascii_uppercase());
        if c > 0 {
            n += 1 + c;
            if let Some(b) = rest[n..].strip_prefix('.') {
                let d = span(b, |c| c.is_ascii_digit());
                if d > 0 {
                    changelist = Some(b[..d].to_string());
                    n += 1 + d;
                }
            }
        }
    }
    Some(Build {
        raw: rest[..n].to_string(),
        changelist,
    })
}

/// "October 9th, 2026" / "June 19, 2025" → date.
fn parse_date(s: &str) -> Option<NaiveDate> {
    const MONTHS: [&str; 12] = [
        "january",
        "february",
        "march",
        "april",
        "may",
        "june",
        "july",
        "august",
        "september",
        "october",
        "november",
        "december",
    ];
    let mut words = s
        .split(|c: char| c.is_whitespace() || c == ',')
        .filter(|w| !w.is_empty());
    let first = words.next()?;
    let month = MONTHS.iter().position(|m| first.eq_ignore_ascii_case(m))? as u32 + 1;
    let day = words
        .next()?
        .trim_end_matches(|c: char| c.is_ascii_alphabetic())
        .parse()
        .ok()?;
    let year = words.next()?.parse().ok()?;
    NaiveDate::from_ymd_opt(year, month, day)
}

// ---------------------------------------------------------------------------
// Content HTML → blocks, via a tiny tolerant DOM

enum Node {
    Elem {
        name: String,
        href: Option<String>,
        children: Vec<Node>,
    },
    Text(String),
}

/// Nesting cap — deeper markup is flattened into its ancestor, so adversarial
/// input can't recurse the walkers off the stack.
const MAX_DEPTH: usize = 64;

fn is_void(name: &str) -> bool {
    matches!(
        name,
        "br" | "img" | "hr" | "input" | "meta" | "link" | "source" | "wbr" | "col" | "area"
    )
}

fn dom(html: &str) -> Vec<Node> {
    // Stack of open elements: (name, href, children); index 0 is the root.
    let mut stack: Vec<(String, Option<String>, Vec<Node>)> =
        vec![(String::new(), None, Vec::new())];
    for tok in tokens(html) {
        match tok {
            Tok::Text(t) => {
                if let Some(top) = stack.last_mut() {
                    top.2.push(Node::Text(decode_entities(t)));
                }
            }
            Tok::Start { name, attrs } => {
                let href = attrs.into_iter().find(|(k, _)| k == "href").map(|(_, v)| v);
                if is_void(&name) || stack.len() > MAX_DEPTH {
                    if let Some(top) = stack.last_mut() {
                        top.2.push(Node::Elem {
                            name,
                            href,
                            children: Vec::new(),
                        });
                    }
                } else {
                    stack.push((name, href, Vec::new()));
                }
            }
            Tok::End { name } => {
                // Close up to the matching open element; stray end tags are ignored.
                if let Some(pos) = stack.iter().rposition(|(n, _, _)| *n == name) {
                    while stack.len() > pos.max(1) {
                        close(&mut stack);
                    }
                }
            }
        }
    }
    while stack.len() > 1 {
        close(&mut stack);
    }
    stack.pop().map(|(_, _, c)| c).unwrap_or_default()
}

fn close(stack: &mut Vec<(String, Option<String>, Vec<Node>)>) {
    if let Some((name, href, children)) = stack.pop() {
        if let Some(parent) = stack.last_mut() {
            parent.2.push(Node::Elem {
                name,
                href,
                children,
            });
        }
    }
}

fn heading_level(name: &str) -> Option<u8> {
    match name.as_bytes() {
        [b'h', d @ b'1'..=b'6'] => Some(d - b'0'),
        _ => None,
    }
}

fn is_inline(name: &str) -> bool {
    matches!(
        name,
        "a" | "strong"
            | "b"
            | "em"
            | "i"
            | "u"
            | "s"
            | "span"
            | "sup"
            | "sub"
            | "code"
            | "small"
            | "mark"
            | "font"
            | "abbr"
            | "img"
    )
}

/// Text of a subtree; block-ish boundaries and `<br>` become spaces.
fn node_text(nodes: &[Node], out: &mut String) {
    for n in nodes {
        match n {
            Node::Text(t) => out.push_str(t),
            Node::Elem { name, children, .. } => {
                if !is_inline(name) {
                    out.push(' ');
                }
                node_text(children, out);
                if !is_inline(name) {
                    out.push(' ');
                }
            }
        }
    }
}

fn text_of(nodes: &[Node]) -> String {
    let mut s = String::new();
    node_text(nodes, &mut s);
    squash(&s)
}

fn blocks(html: &str) -> Vec<Block> {
    let tree = dom(html);
    let mut w = Walker {
        out: Vec::new(),
        inline: Vec::new(),
        last_level: 3,
    };
    w.walk(&tree);
    w.flush();
    w.out
}

struct Walker<'a> {
    out: Vec<Block>,
    /// Pending inline run at block level (bare text outside any `<p>`).
    inline: Vec<&'a Node>,
    /// Level of the last heading, for bold-only pseudo-headings.
    last_level: u8,
}

impl<'a> Walker<'a> {
    fn walk(&mut self, nodes: &'a [Node]) {
        for n in nodes {
            let Node::Elem { name, children, .. } = n else {
                self.inline.push(n);
                continue;
            };
            if is_inline(name) {
                self.inline.push(n);
                continue;
            }
            self.flush();
            if let Some(level) = heading_level(name) {
                let text = text_of(children);
                if !text.is_empty() {
                    self.last_level = level;
                    self.out.push(Block::Heading { level, text });
                }
                continue;
            }
            match name.as_str() {
                "p" => self.paragraph(children.iter().collect()),
                "ul" | "ol" => {
                    let items = list_items(children);
                    if !items.is_empty() {
                        self.out.push(Block::List(items));
                    }
                }
                "li" => {
                    let item = li_text(children);
                    if !item.is_empty() {
                        self.out.push(Block::List(vec![item]));
                    }
                }
                "br" | "hr" | "script" | "style" => {}
                _ => self.walk(children),
            }
        }
    }

    fn flush(&mut self) {
        let run = std::mem::take(&mut self.inline);
        if !run.is_empty() {
            self.paragraph(run);
        }
    }

    fn paragraph(&mut self, nodes: Vec<&Node>) {
        // Ignore whitespace text and <br> when looking at the shape.
        let meaningful: Vec<&Node> = nodes
            .iter()
            .copied()
            .filter(|n| match n {
                Node::Text(t) => !squash(t).is_empty(),
                Node::Elem { name, .. } => name != "br",
            })
            .collect();
        let text = squash(
            &nodes
                .iter()
                .map(|n| text_of(std::slice::from_ref(n)))
                .collect::<Vec<_>>()
                .join(" "),
        );
        if text.is_empty() {
            return;
        }
        if let [Node::Elem {
            name,
            href: Some(url),
            children,
        }] = meaningful.as_slice()
        {
            if name == "a" {
                self.out.push(Block::Link {
                    text: text_of(children),
                    url: url.clone(),
                });
                return;
            }
        }
        let first_bold = matches!(meaningful.first(), Some(Node::Elem { name, .. }) if name == "strong" || name == "b");
        if first_bold {
            let lead = text_of(std::slice::from_ref(meaningful[0]));
            let rest = squash(
                &meaningful[1..]
                    .iter()
                    .map(|n| text_of(std::slice::from_ref(*n)))
                    .collect::<Vec<_>>()
                    .join(" "),
            );
            if rest.is_empty() {
                if lead.chars().count() <= 80 && !lead.ends_with('.') {
                    // ≥ 4 keeps pseudo-headings out of the highlights.
                    let level = (self.last_level + 1).clamp(4, 6);
                    self.out.push(Block::Heading { level, text: lead });
                    return;
                }
            } else if !lead.is_empty() {
                self.out.push(Block::Paragraph {
                    lead: Some(lead),
                    text: rest,
                });
                return;
            }
        }
        self.out.push(Block::Paragraph { lead: None, text });
    }
}

fn list_items(children: &[Node]) -> Vec<String> {
    children
        .iter()
        .filter_map(|n| match n {
            Node::Elem { name, children, .. } if name == "li" => Some(li_text(children)),
            _ => None,
        })
        .filter(|t| !t.is_empty())
        .collect()
}

/// An item's own text, then its nested items one per line.
fn li_text(children: &[Node]) -> String {
    let mut own = String::new();
    let mut nested = Vec::new();
    for n in children {
        match n {
            Node::Elem { name, children, .. } if name == "ul" || name == "ol" => {
                nested.extend(list_items(children))
            }
            _ => node_text(std::slice::from_ref(n), &mut own),
        }
    }
    let own = squash(&own);
    std::iter::once(own)
        .chain(nested)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Plain text of an HTML snippet, whitespace collapsed.
fn plain_text(html: &str) -> String {
    text_of(&dom(html))
}

/// Text with tags dropped without spacing — keeps "LIVE.<strong>9873572"
/// whole for the VERSION line.
fn inline_text(html: &str) -> String {
    let s: String = tokens(html)
        .into_iter()
        .filter_map(|t| match t {
            Tok::Text(t) => Some(decode_entities(t)),
            _ => None,
        })
        .collect();
    squash(&s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = include_str!("../tests/fixtures/patchnotes/index.html");
    const SHELL_4_10_2: &str = include_str!("../tests/fixtures/patchnotes/4.10.2.shell.html");
    const BODY_4_10_2: &str = include_str!("../tests/fixtures/patchnotes/4.10.2.body.html");
    const BODY_4_10_1: &str = include_str!("../tests/fixtures/patchnotes/4.10.1.body.html");
    const BODY_4_8: &str = include_str!("../tests/fixtures/patchnotes/4.8.body.html");
    const BODY_4_2: &str = include_str!("../tests/fixtures/patchnotes/4.2.body.html");

    const LOREM: [&str; 6] = [
        "lorem",
        "ipsum",
        "consectetur",
        "adipiscing",
        "dolor sit",
        "perspiciatis",
    ];
    const BOILERPLATE: [&str; 4] = [
        "RSI Launcher",
        "Share Feedback",
        "Join the Discussion",
        "Report a Bug",
    ];

    fn ymd(y: i32, m: u32, d: u32) -> Option<NaiveDate> {
        NaiveDate::from_ymd_opt(y, m, d)
    }

    fn one(fragment: &str) -> Release {
        let mut r = parse_releases(fragment);
        assert_eq!(r.len(), 1);
        r.remove(0)
    }

    fn all_text(blocks: &[Block], out: &mut String) {
        for b in blocks {
            match b {
                Block::Heading { text, .. } | Block::Link { text, .. } => out.push_str(text),
                Block::Paragraph { lead, text } => {
                    out.push_str(lead.as_deref().unwrap_or_default());
                    out.push_str(text);
                }
                Block::List(items) => items.iter().for_each(|i| out.push_str(i)),
                Block::Entry { title, blocks } => {
                    out.push_str(title);
                    all_text(blocks, out);
                }
            }
            out.push('\n');
        }
    }

    fn assert_clean(r: &Release) {
        assert!(
            r.date.is_some(),
            "{}: no date from {:?}",
            r.title,
            r.date_text
        );
        assert!(r.version.is_some(), "{}: no version", r.title);
        assert!(r.build.is_some(), "{}: no build", r.title);
        assert!(!r.highlights.is_empty(), "{}: no highlights", r.title);
        for h in &r.highlights {
            let l = h.to_lowercase();
            assert!(!h.trim().is_empty(), "{}: empty highlight", r.title);
            assert!(
                !LOREM.iter().any(|w| l.contains(w)),
                "{}: lorem highlight {h:?}",
                r.title
            );
            assert!(
                !BOILERPLATE.iter().any(|w| h == w),
                "{}: boilerplate highlight {h:?}",
                r.title
            );
        }
        let mut text = String::new();
        for s in &r.sections {
            all_text(&s.blocks, &mut text);
        }
        let l = text.to_lowercase();
        assert!(
            !LOREM.iter().any(|w| l.contains(w)),
            "{}: lorem in sections",
            r.title
        );
    }

    #[test]
    fn index_page() {
        let refs = parse_index(INDEX, PROD_ROOT);
        assert_eq!(refs.len(), 20);
        let r = &refs[0];
        assert_eq!(r.id, 21351);
        assert_eq!(r.title, "Star Citizen Alpha 4.10.2");
        assert_eq!(r.version.as_deref(), Some("4.10.2"));
        assert_eq!(
            r.url,
            "https://robertsspaceindustries.com/comm-link/Patch-Notes/21351-Star-Citizen-Alpha-4102"
        );
        assert!(r
            .thumbnail
            .as_deref()
            .unwrap()
            .ends_with("/heap_infobox.jpg"));
        assert!(r.age_text.ends_with("ago"));
        let last = refs.last().unwrap();
        assert_eq!((last.id, last.version.as_deref()), (20309, Some("3.24.3")));
        assert!(refs.iter().all(|r| r.version.is_some()));
    }

    #[test]
    fn shell_points_at_the_fragment() {
        let url = alexandria_url(SHELL_4_10_2).unwrap();
        assert!(url.starts_with("https://robertsspaceindustries.com/alexandria/html/"));
        assert_eq!(
            og_title(SHELL_4_10_2).as_deref(),
            Some("Star Citizen Alpha 4.10.2")
        );
        assert_eq!(
            article_id("https://x/en/comm-link/Patch-Notes/21351-Star-Citizen-Alpha-4102"),
            Some(21351)
        );
    }

    #[test]
    fn faq_template_4_10_1() {
        let r = one(BODY_4_10_1);
        assert_clean(&r);
        assert_eq!(r.title, "Star Citizen Alpha 4.10.1 LIVE");
        assert_eq!(r.date, ymd(2026, 9, 16));
        assert_eq!(r.version.as_deref(), Some("4.10.1"));
        assert_eq!(r.channel.as_deref(), Some("LIVE"));
        assert_eq!(r.name.as_deref(), Some("Orison Relief Support"));
        assert_eq!(
            r.counts,
            Counts {
                bug_fixes: Some(57),
                known_issues: None
            }
        );
        let b = r.build.as_ref().unwrap();
        assert_eq!(
            (b.raw.as_str(), b.changelist.as_deref()),
            ("4.10.1-LIVE.12650677", Some("12650677"))
        );
        assert!(r
            .build_info
            .contains(&("Starting aUEC".into(), "20,000".into())));
        assert_eq!(
            r.highlights,
            ["Gameplay: Orison Relief Support - Ground Vehicle Soft Death - Loot Quality and Attachment Availability"]
        );
        let kinds: Vec<_> = r.sections.iter().map(|s| s.kind).collect();
        assert_eq!(kinds, [SectionKind::Features, SectionKind::BugFixes]);
        let Block::Entry { blocks, .. } = &r.sections[0].blocks[0] else {
            panic!("features should be a FAQ entry");
        };
        assert!(blocks.contains(&Block::Heading {
            level: 4,
            text: "Ground Vehicle Soft Death".into()
        }));
        assert!(r.sections[1]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::List(i) if i.len() > 5)));
    }

    #[test]
    fn minigrid_artemis_template_4_10_2() {
        let r = one(BODY_4_10_2);
        assert_clean(&r);
        assert_eq!(r.date, ymd(2026, 10, 9));
        assert_eq!(r.name.as_deref(), Some("RSI Discovery Month"));
        // Known issues are only a KB link inside the bug-fix narrative.
        assert_eq!(
            r.counts,
            Counts {
                bug_fixes: Some(89),
                known_issues: None
            }
        );
        assert_eq!(r.build.as_ref().unwrap().raw, "4.10.2-LIVE.12881860");
        assert_eq!(
            r.highlights,
            [
                "RSI Discovery Month",
                "RSI Constellation Mk IV",
                "Physics Networking Overhaul"
            ]
        );
        let feat = &r.sections[0].blocks;
        assert!(feat
            .iter()
            .any(|b| matches!(b, Block::Paragraph { lead: Some(l), .. } if l == "Bridge:")));
        let fixes = &r.sections[1];
        assert_eq!(fixes.kind, SectionKind::BugFixes);
        assert!(fixes.blocks.contains(&Block::Heading {
            level: 2,
            text: "Hotfixes Already Applied to Live".into()
        }));
        // Bold-only paragraph → sub-heading under the h3 before it.
        assert!(fixes.blocks.contains(&Block::Heading {
            level: 4,
            text: "Orison Relief Support".into()
        }));
        // Nested explanation bullets stay with their fix.
        let nested = fixes.blocks.iter().any(|b| match b {
            Block::List(items) => items.iter().any(|i| {
                i.starts_with("Resolved an issue where Orison Relief Support salvage contracts")
                    && i.contains("\nThe hand in quantities")
            }),
            _ => false,
        });
        assert!(nested);
    }

    #[test]
    fn accumulating_article_4_8() {
        let rs = parse_releases(BODY_4_8);
        let builds: Vec<_> = rs
            .iter()
            .map(|r| r.build.as_ref().unwrap().raw.as_str())
            .collect();
        assert_eq!(
            builds,
            [
                "4.8.3-LIVE.12122953",
                "4.8.2-LIVE.12030094",
                "4.8.1-LIVE.11875683",
                "4.8.0-LIVE.1825000"
            ]
        );
        // CIG's 7-digit typo on 4.8.0, kept as published.
        assert_eq!(
            rs[3].build.as_ref().unwrap().changelist.as_deref(),
            Some("1825000")
        );
        assert_eq!(rs[0].date, ymd(2026, 7, 1));
        assert_eq!(rs[3].date, ymd(2026, 5, 13));
        assert_eq!(rs[1].name.as_deref(), Some("Update"));
        assert_eq!(rs[1].sections[0].title, IMPLICIT_SECTION);
        assert_eq!(rs[1].highlights, ["4.8.2: Bugfixes & Technical"]);
        assert_eq!(rs[1].counts, Counts::default());
        assert_eq!(
            rs[3].counts,
            Counts {
                bug_fixes: Some(70),
                known_issues: Some(13)
            }
        );
        assert_eq!(rs[3].version.as_deref(), Some("4.8"));
        assert_eq!(
            rs[3].build_info[0],
            (
                "Long Term Persistence".into(),
                "LTP wipe while preserving blueprints.".into()
            )
        );
        rs.iter().for_each(assert_clean);
    }

    #[test]
    fn split_version_line_4_2() {
        let r = one(BODY_4_2);
        assert_clean(&r);
        assert_eq!(r.build.as_ref().unwrap().raw, "4.2.0-LIVE.9873572");
        assert_eq!(r.date, ymd(2025, 6, 19));
        assert_eq!(
            r.counts,
            Counts {
                bug_fixes: Some(58),
                known_issues: Some(13)
            }
        );
        let known = r.sections.last().unwrap();
        assert_eq!(known.kind, SectionKind::KnownIssues);
        assert!(known.blocks.iter().any(
            |b| matches!(b, Block::Link { url, .. } if url.contains("support.robertsspaceindustries.com"))
        ));
        assert!(r
            .build_info
            .contains(&("Server Meshing".into(), "5:5:600 Player Shards".into())));
    }

    #[test]
    fn helpers() {
        assert_eq!(parse_date("October 9th, 2026"), ymd(2026, 10, 9));
        assert_eq!(parse_date("June 19, 2025"), ymd(2025, 6, 19));
        assert_eq!(parse_date("February 30, 2025"), None);
        assert_eq!(parse_date("soon"), None);
        let b = find_build("VERSION 4.10.2-LIVE.12881860Reminder.").unwrap();
        assert_eq!(
            (b.raw.as_str(), b.changelist.as_deref()),
            ("4.10.2-LIVE.12881860", Some("12881860"))
        );
        assert_eq!(find_build("VERSION: 4.0.1. Next").unwrap().raw, "4.0.1");
        assert!(find_build("VERSION soon").is_none());
        assert_eq!(
            title_version("Star Citizen Alpha 3.24.3"),
            Some("3.24.3".into())
        );
        assert_eq!(
            title_channel("Star Citizen Alpha 4.10 LIVE"),
            Some("LIVE".into())
        );
        let items = build_info("<p>Items not saved to LTP Include:</p><ul><li>Medpens</li></ul><ul><li>Starting aUEC: 20,000</li></ul>");
        assert_eq!(
            items,
            [
                (
                    "Items not saved to LTP Include".to_string(),
                    "Medpens".to_string()
                ),
                ("Starting aUEC".to_string(), "20,000".to_string())
            ]
        );
    }

    #[test]
    fn adversarial_input_does_not_panic() {
        let deep = "<div>".repeat(10_000);
        for s in [
            "<",
            "<a",
            "<a b=\"",
            "&#xFFFFFFFF;",
            "&#xD800;",
            "<h",
            "</",
            "<!--",
            "&",
            "<a =é>",
            "<p é=\"x\">",
            "<g-faq :question-list='[{\"title\":1}]'>",
            deep.as_str(),
        ] {
            let _ = tokens(s);
            let _ = blocks(s);
            let _ = parse_releases(s);
            let _ = parse_index(s, PROD_ROOT);
            let _ = alexandria_url(s);
        }
    }

    #[test]
    fn types_round_trip_through_json() {
        let r = parse_releases(BODY_4_10_2);
        let json = serde_json::to_string(&r).unwrap();
        let back: Vec<Release> = serde_json::from_str(&json).unwrap();
        assert_eq!(r, back);
    }

    /// Every index entry, live: each release has a version and a date, and no
    /// highlight is empty, lorem, or boilerplate. ~40 requests, paced.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_all_index_entries() {
        let pace = std::time::Duration::from_millis(1500);
        let notes = PatchNotes::new("sc-dossier-test");
        let index = notes.index().await.unwrap();
        assert!(index.len() >= 20);
        for r in &index {
            tokio::time::sleep(pace).await;
            let a = notes.article(&r.url).await.unwrap();
            assert!(!a.releases.is_empty(), "{}", r.url);
            a.releases.iter().for_each(assert_clean);
        }
    }
}
