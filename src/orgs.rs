//! Player organizations from the public RSI website, anonymous.
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! use sc_dossier::orgs::{MemberQuery, Orgs};
//! let orgs = Orgs::new("my-app/1.0");
//! for m in orgs.citizen_orgs("VeeLume").await? {
//!     println!("{:?} {:?} rank {:?} ({}★)", m.sid, m.name, m.rank, m.stars);
//! }
//! let org = orgs.org("TEST").await?;
//! let page = orgs.members("TEST", &MemberQuery::default()).await?;
//! println!("{} has {} members", org.name, page.total);
//! # Ok(()) }
//! ```
//!
//! - [`Orgs::citizen_orgs`] — one GET of `/citizens/<handle>/organizations`.
//! - [`Orgs::org`] — one GET of `/orgs/<SID>`.
//! - [`Orgs::members`] — one `POST /api/orgs/getOrgMembers`, 32 per page. Big
//!   orgs take hundreds of pages; paging is the caller's.
//!
//! Members and affiliations can be redacted or hidden by their owner: those
//! keep their stars but carry no handle, name, or rank.
//!
//! No pacing, caching, or retries — the caller's policy. The rate limit is
//! unmeasured; stay under ~1 request/second.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::html::{between, decode_entities, squash, text};

/// Production host.
pub const PROD_ROOT: &str = "https://robertsspaceindustries.com";

/// Members per page; the website's page size.
const MEMBER_PAGE: u32 = 32;

/// Anonymous client for the org pages.
#[derive(Debug, Clone)]
pub struct Orgs {
    http: reqwest::Client,
    root: String,
    user_agent: String,
}

impl Orgs {
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

    /// A citizen's main org and affiliations, in page order (main first). An
    /// unknown handle is an [`Error::Page`] with HTTP 404.
    pub async fn citizen_orgs(&self, handle: &str) -> Result<Vec<Membership>> {
        let url = format!("{}/en/citizens/{handle}/organizations", self.root);
        Ok(parse_citizen_orgs(&self.get(&url).await?, &self.root))
    }

    /// One org's profile by SID. An unknown SID is an [`Error::Page`] with
    /// HTTP 404.
    pub async fn org(&self, sid: &str) -> Result<Org> {
        let url = format!("{}/en/orgs/{sid}", self.root);
        parse_org(&self.get(&url).await?, &self.root)
            .ok_or_else(|| page_err(&url, "no org profile"))
    }

    /// One page of an org's members.
    pub async fn members(&self, sid: &str, query: &MemberQuery) -> Result<MemberPage> {
        let url = format!("{}/api/orgs/getOrgMembers", self.root);
        let page = query.page.max(1);
        let resp = self
            .http
            .post(&url)
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .json(&json!({
                "symbol": sid,
                "search": query.search.as_deref().unwrap_or_default(),
                "pagesize": MEMBER_PAGE,
                "page": page,
            }))
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
        parse_members(&env, page, &self.root).map_err(|what| page_err(&url, &what))
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

/// Who may see a membership, as its owner set it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Visibility {
    /// Shown (`V`).
    Visible,
    /// Redacted (`R`): listed, details blanked.
    Redacted,
    /// Hidden (`H`): listed, details blanked.
    Hidden,
    /// Any other code.
    Other(String),
}

impl Visibility {
    fn from_code(c: &str) -> Self {
        match c {
            "V" => Self::Visible,
            "R" => Self::Redacted,
            "H" => Self::Hidden,
            other => Self::Other(other.to_string()),
        }
    }
}

/// One of a citizen's orgs. Blanked ones ([`Visibility`]) have no SID,
/// name, or rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Membership {
    /// The citizen's main org (one at most) rather than an affiliation.
    pub is_main: bool,
    /// Visibility.
    pub visibility: Visibility,
    /// Spectrum id (`"TEST"`).
    pub sid: Option<String>,
    /// Org name.
    pub name: Option<String>,
    /// Logo URL.
    pub logo: Option<String>,
    /// Member count.
    pub members: Option<u32>,
    /// The citizen's rank title there (org-defined: `"Wingman"`, `"Recruit"`).
    pub rank: Option<String>,
    /// Rank level, 0–5 stars.
    pub stars: u8,
}

/// An org's public profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Org {
    /// Spectrum id.
    pub sid: String,
    /// Name.
    pub name: String,
    /// Logo URL.
    pub logo: Option<String>,
    /// Banner URL.
    pub banner: Option<String>,
    /// Member count.
    pub members: Option<u32>,
    /// Archetype: `"Organization"`, `"Corporation"`, `"PMC"`, `"Faith"`, ….
    pub archetype: Option<String>,
    /// `"Casual"`, `"Regular"`, `"Hardcore"`.
    pub commitment: Option<String>,
    /// Tagged as role play.
    pub roleplay: bool,
    /// Primary activity (`"Social"`, `"Security"`, …).
    pub focus_primary: Option<String>,
    /// Secondary activity.
    pub focus_secondary: Option<String>,
    /// The "join us" intro, text only (may be empty).
    pub intro: String,
    /// History tab, text only.
    pub history: String,
    /// Manifesto tab, text only.
    pub manifesto: String,
    /// Charter tab, text only.
    pub charter: String,
}

/// Member-list filters. `Default` = page 1, everyone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberQuery {
    /// 1-based.
    pub page: u32,
    /// Handle or name search.
    pub search: Option<String>,
}

impl Default for MemberQuery {
    fn default() -> Self {
        Self {
            page: 1,
            search: None,
        }
    }
}

/// One page of members.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberPage {
    /// Members matching the query, all pages (blanked ones included).
    pub total: u32,
    /// Up to 32.
    pub members: Vec<Member>,
    /// No pages after this one.
    pub is_last: bool,
}

/// An org member. Blanked ones ([`Visibility`]) have no handle, name, rank,
/// or roles.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Citizen handle (the profile URL's key).
    pub handle: Option<String>,
    /// Display name.
    pub display_name: Option<String>,
    /// Rank title (org-defined).
    pub rank: Option<String>,
    /// Rank level, 0–5 stars.
    pub stars: u8,
    /// Org roles (`"Founder"`, `"Officer"`, `"Recruitment"`, custom).
    pub roles: Vec<String>,
    /// Avatar URL.
    pub avatar: Option<String>,
    /// This org is the member's main org.
    pub is_main: bool,
    /// Visibility.
    pub visibility: Visibility,
}

/// `/x` → `<root>/x`; absolute URLs pass through.
fn absolute(url: &str, root: &str) -> String {
    if url.starts_with('/') && !url.starts_with("//") {
        format!("{root}{url}")
    } else {
        url.to_string()
    }
}

/// Text in `s` between `start` and `end`, entity-decoded and squashed;
/// `None` when missing or blank.
fn field(s: &str, start: &str, end: &str) -> Option<String> {
    let t = text(between(s, start, end)?);
    (!t.is_empty()).then_some(t)
}

/// `"24601 members"` → 24601.
fn count(s: &str) -> Option<u32> {
    s.split_whitespace().next()?.parse().ok()
}

/// The `src` of the first `<img` after `marker`.
fn img_after(s: &str, marker: &str, root: &str) -> Option<String> {
    let i = s.find(marker)?;
    let src = between(&s[i..], "<img src=\"", "\"")?;
    Some(absolute(&decode_entities(src), root))
}

fn parse_citizen_orgs(html: &str, root: &str) -> Vec<Membership> {
    let Some(start) = html.find("orgs-content") else {
        return Vec::new();
    };
    html[start..]
        .split("<div class=\"box-content org ")
        .skip(1)
        .map(|card| {
            let head = card.split('"').next().unwrap_or_default();
            let visibility = head
                .split_whitespace()
                .find_map(|c| c.strip_prefix("visibility-"))
                .map_or(Visibility::Other(String::new()), Visibility::from_code);
            let ranking = between(card, "class=\"ranking", "</div>").unwrap_or_default();
            Membership {
                is_main: head.split_whitespace().any(|c| c == "main"),
                visibility,
                sid: between(card, "href=\"/orgs/", "\"").map(str::to_string),
                name: field(card, "class=\"entry orgtitle\">", "</p>")
                    .or_else(|| field(card, "<p class=\"entry\">", "</p>")),
                logo: img_after(card, "class=\"thumb\"", root),
                members: between(card, "class=\"members\">", "<").and_then(count),
                rank: between(card, "Organization rank</span>", "</strong>")
                    .and_then(|s| s.split_once('>'))
                    .map(|(_, r)| squash(&decode_entities(r)))
                    .filter(|r| !r.is_empty()),
                stars: ranking.matches("class=\"active\"").count().min(5) as u8,
            }
        })
        .collect()
}

fn parse_org(html: &str, root: &str) -> Option<Org> {
    let page = &html[html.find("id=\"organization\"")?..];
    let sid = squash(between(page, "<span class=\"symbol\">", "<")?);
    let h1 = between(page, "<h1>", "<span class=\"symbol\">")?;
    let name = text(h1).trim_end_matches('/').trim_end().to_string();
    let tag = |class: &str| field(page, &format!("<li class=\"{class}\">"), "</li>");
    let focus = |class: &str| {
        let i = page.find(&format!("<li class=\"{class} tooltip-wrap\">"))?;
        let alt = between(&page[i..], "alt=\"", "\"")?;
        Some(decode_entities(alt)).filter(|a| !a.is_empty())
    };
    let tab = |id: &str, end: &str| {
        // Leading space: the nav links carry `data-content_id="tab-…"`.
        let i = page.find(&format!(" id=\"tab-{id}\""))?;
        let body = &page[i..];
        let body = &body[body.find("class=\"markitup-text\">")? + 22..];
        Some(text(&body[..body.find(end).unwrap_or(body.len())]))
    };
    Some(Org {
        sid,
        name,
        logo: img_after(page, "<div class=\"logo", root),
        banner: img_after(page, "<div class=\"banner\">", root),
        members: between(page, "<span class=\"count\">", "<").and_then(count),
        archetype: tag("model"),
        commitment: tag("commitment"),
        roleplay: page.contains("<li class=\"roleplay\">"),
        focus_primary: focus("primary"),
        focus_secondary: focus("secondary"),
        intro: between(
            page,
            "class=\"body markitup-text\">",
            "class=\"frame bottom\"",
        )
        .map(text)
        .unwrap_or_default(),
        history: tab("history", " id=\"tab-manifesto\"").unwrap_or_default(),
        manifesto: tab("manifesto", " id=\"tab-charter\"").unwrap_or_default(),
        charter: tab("charter", "class=\"content block").unwrap_or_default(),
    })
}

/// `{success, code, msg, data: {totalrows, html}}` → page. `Err` names what
/// was wrong; an envelope error carries its code (`ErrInvalidOrganization`).
fn parse_members(env: &Value, page: u32, root: &str) -> std::result::Result<MemberPage, String> {
    if env.get("success").and_then(Value::as_i64) != Some(1) {
        let code = env.get("code").and_then(Value::as_str).unwrap_or("?");
        let msg = env.get("msg").and_then(Value::as_str).unwrap_or("");
        return Err(format!("{code}: {msg}"));
    }
    let data = env.get("data").ok_or("envelope has no data")?;
    let total = data
        .get("totalrows")
        .and_then(|t| t.as_u64().or_else(|| t.as_str()?.parse().ok()))
        .and_then(|t| u32::try_from(t).ok())
        .ok_or("no totalrows")?;
    let html = data.get("html").and_then(Value::as_str).unwrap_or_default();
    let members: Vec<Member> = html
        .split("<li class=\"member-item")
        .skip(1)
        .map(|card| parse_member(card, root))
        .collect();
    Ok(MemberPage {
        is_last: members.is_empty() || u64::from(page) * u64::from(MEMBER_PAGE) >= u64::from(total),
        total,
        members,
    })
}

fn parse_member(card: &str, root: &str) -> Member {
    let head = card.split('"').next().unwrap_or_default();
    let visibility = head
        .split_whitespace()
        .find_map(|c| c.strip_prefix("org-visibility-"))
        .map_or(Visibility::Other(String::new()), Visibility::from_code);
    let stars = between(card, "class=\"stars\" style=\"width:", "%")
        .and_then(|w| w.trim().parse::<u32>().ok())
        .map_or(0, |w| (w / 20).min(5) as u8);
    let roles = between(card, "<ul class=\"rolelist\">", "</ul>")
        .unwrap_or_default()
        .split("<li class=\"role\">")
        .skip(1)
        .map(|r| text(r.split("</li>").next().unwrap_or_default()))
        .filter(|r| !r.is_empty())
        .collect();
    Member {
        handle: between(card, "href=\"/citizens/", "\"").map(decode_entities),
        // `<span class="trans-03s name dataN">Name</span>`; blanked = nbsp only.
        display_name: between(card, "class=\"trans-03s name", "</span>")
            .and_then(|s| s.split_once('>'))
            .map(|(_, n)| squash(&decode_entities(n)))
            .filter(|n| !n.is_empty()),
        rank: field(card, "<span class=\"rank\">", "</span>"),
        stars,
        roles,
        avatar: img_after(card, "<span class=\"thumb\">", root),
        is_main: head.split_whitespace().any(|c| c == "org-main"),
        visibility,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ORG_TEST: &str = include_str!("../tests/fixtures/orgs/org_TEST.html");
    const CITIZEN: &str = include_str!("../tests/fixtures/orgs/citizen_VeeLume.html");

    #[test]
    fn org_profile() {
        let o = parse_org(ORG_TEST, PROD_ROOT).unwrap();
        assert_eq!(o.sid, "TEST");
        assert_eq!(o.name, "TEST Squadron - Best Squardon!");
        assert_eq!(o.members, Some(24601));
        assert_eq!(
            o.logo.as_deref(),
            Some("https://robertsspaceindustries.com/media/ihh8ge4y6eqfir/logo/TEST-Logo.png")
        );
        assert!(o.banner.as_deref().unwrap().ends_with("/TEST-Banner.png"));
        assert_eq!(
            (o.archetype.as_deref(), o.commitment.as_deref(), o.roleplay),
            (Some("Organization"), Some("Casual"), false)
        );
        assert_eq!(
            (o.focus_primary.as_deref(), o.focus_secondary.as_deref()),
            (Some("Social"), Some("Security"))
        );
        assert!(o.intro.starts_with("Welcome Test Pilot!"), "{}", o.intro);
        // Inline tags become spaces: `TEST</span>?` → "TEST ?".
        assert!(o.history.starts_with("Why Join TEST ?"), "{}", o.history);
        assert!(o.history.ends_with("18+ ONLY"), "{}", o.history);
        assert!(o.manifesto.starts_with('\u{201c}'), "{}", o.manifesto);
        assert!(!o.manifesto.contains("Why Join"), "{}", o.manifesto);
        assert!(o
            .charter
            .starts_with("You have the right to crash your Aurora."));
        assert!(!o.charter.contains("Cover"));
    }

    #[test]
    fn citizen_affiliations() {
        let m = parse_citizen_orgs(CITIZEN, PROD_ROOT);
        assert_eq!(m.len(), 3);
        let a = &m[0];
        assert!(!a.is_main);
        assert_eq!(a.visibility, Visibility::Visible);
        assert_eq!(a.sid.as_deref(), Some("HNDI"));
        assert_eq!(
            a.name.as_deref(),
            Some("Hendicab\u{2019}s Nearly Deadly Idiots")
        );
        assert_eq!(a.members, Some(36));
        assert_eq!(a.rank.as_deref(), Some("Wingman der Vernichtung"));
        assert_eq!(a.stars, 2);
        assert!(a.logo.as_deref().unwrap().starts_with("https://"));
        assert_eq!(
            m.iter()
                .map(|m| m.sid.as_deref().unwrap())
                .collect::<Vec<_>>(),
            ["HNDI", "STARWHAL", "TEMPORG"]
        );
    }

    /// Main-org card as served (2026-10-21), names replaced.
    #[test]
    fn citizen_main_org() {
        let html = r#"<div class="profile-content orgs-content clearfix">
          <div class="box-content org main visibility-V">
            <div class="title">Main organization </div>
            <div class="thumb">
              <a href="/orgs/TEST"><img src="/media/ihh8ge4y6eqfir/heap_infobox/TEST-Logo.png" /></a>
              <span class="members">24601 members</span>
            </div>
            <div class="info">
              <p class="entry">
                <a href="/orgs/TEST" class="value">TEST Squadron - Best Squardon!</a>
              </p>
              <p class="entry">
                <span class="label">Spectrum Identification (SID)</span>
                <strong class="value">TEST</strong>
              </p>
              <p class="entry">
                <span class="label">Organization rank</span>
                <strong class="value">Cadet</strong>
              </p>
              <div class="ranking">
                <span ><span></span></span><span ><span></span></span>
              </div>
            </div>
          </div>
          <div class="box-content org affiliation visibility-H">
            <div class="title">Affiliation</div>
          </div>"#;
        let m = parse_citizen_orgs(html, PROD_ROOT);
        assert_eq!(m.len(), 2);
        assert!(m[0].is_main);
        assert_eq!(m[0].name.as_deref(), Some("TEST Squadron - Best Squardon!"));
        assert_eq!(m[0].rank.as_deref(), Some("Cadet"));
        assert_eq!(m[0].stars, 0);
        assert_eq!(
            m[0].logo.as_deref(),
            Some("https://robertsspaceindustries.com/media/ihh8ge4y6eqfir/heap_infobox/TEST-Logo.png")
        );
        let h = &m[1];
        assert_eq!(
            (h.is_main, &h.visibility, &h.sid, &h.name, &h.rank),
            (false, &Visibility::Hidden, &None, &None, &None)
        );
    }

    /// Cards as served (2026-10-21), names replaced.
    fn members_env(total: u32) -> Value {
        let html = r#"	<li class="member-item js-member-item org-main org-visibility-V" data-org-sid="TEST" data-org-name="TEST Squadron">
		<a class="membercard js-edit-member"  href="/citizens/Some_Pilot">
      <span class="thumb">
        				<img src="https://cdn.robertsspaceindustries.com/static/images/account/avatar_default_big.jpg" />
			</span>
			<span class="right">
        				<span class="abs-overlay trans-03s roles">
          					<span class="title">Roles</span>
					<ul class="rolelist">
						<li class="role">Founder</li>
						<li class="role">Officer</li>
					</ul>
          				</span>
				<span class="trans-03s frontinfo">
					<span class="name-wrap">
						<span class="trans-03s name data4">Some Pilot</span>
						<span class="trans-03s nick data3">Some_Pilot</span>
					</span>
					<span class="ranking-stars data1">
						<span class="stars" style="width: 100%;"></span>
					</span>
					<span class="rank">Admiral</span>
				</span>
			</span>
		</a>
	</li>
	<li class="member-item js-member-item org-visibility-H" data-org-sid="TEST" data-org-name="TEST Squadron">
		<a class="membercard js-edit-member" >
			<span class="thumb">
			</span>
			<span class="right">
				<span class="trans-03s frontinfo">
					<span class="name-wrap">
						<span class="trans-03s name data5">&nbsp;&nbsp;&nbsp;&nbsp;</span>
						<span class="trans-03s nick data5">&nbsp;&nbsp;&nbsp;</span>
					</span>
					<span class="ranking-stars data4">
						<span class="stars" style="width: 20%;"></span>
					</span>
				</span>
			</span>
		</a>
		<div class="member-visibility-restriction member-visibility-H trans-03s"></div>
	</li>"#;
        json!({"success": 1, "code": "OK", "msg": "OK", "data": {"totalrows": total, "html": html}})
    }

    #[test]
    fn member_cards() {
        let p = parse_members(&members_env(36), 1, PROD_ROOT).unwrap();
        assert_eq!((p.total, p.members.len(), p.is_last), (36, 2, false));
        let m = &p.members[0];
        assert_eq!(m.handle.as_deref(), Some("Some_Pilot"));
        assert_eq!(m.display_name.as_deref(), Some("Some Pilot"));
        assert_eq!(m.rank.as_deref(), Some("Admiral"));
        assert_eq!(m.stars, 5);
        assert_eq!(m.roles, ["Founder", "Officer"]);
        assert!(m
            .avatar
            .as_deref()
            .unwrap()
            .ends_with("avatar_default_big.jpg"));
        assert!(m.is_main);
        assert_eq!(m.visibility, Visibility::Visible);
        let h = &p.members[1];
        assert_eq!(
            (&h.handle, &h.display_name, &h.rank, h.stars, h.is_main),
            (&None, &None, &None, 1, false)
        );
        assert_eq!(
            (h.visibility.clone(), h.avatar.clone()),
            (Visibility::Hidden, None)
        );
        assert!(h.roles.is_empty());
        assert!(
            parse_members(&members_env(36), 2, PROD_ROOT)
                .unwrap()
                .is_last
        );
    }

    #[test]
    fn member_envelopes() {
        let empty = json!({"success": 1, "data": {"totalrows": 36, "html": ""}});
        let p = parse_members(&empty, 3, PROD_ROOT).unwrap();
        assert!(p.is_last && p.members.is_empty());
        let bad = json!({"success": 0, "code": "ErrInvalidOrganization",
                         "msg": "Invalid Organization.", "data": null});
        assert_eq!(
            parse_members(&bad, 1, PROD_ROOT).unwrap_err(),
            "ErrInvalidOrganization: Invalid Organization."
        );
        assert!(parse_members(&json!({"success": 1, "data": {}}), 1, PROD_ROOT).is_err());
    }

    #[test]
    fn adversarial_input_does_not_panic() {
        for s in [
            "",
            "id=\"organization\"",
            "id=\"organization\"<h1><span class=\"symbol\">",
            "id=\"organization\"<span class=\"symbol\">X<h1>é<span class=\"symbol\">id=\"tab-history\"",
            "id=\"organization\"<h1>a<span class=\"symbol\">B</span>id=\"tab-history\"class=\"markitup-text\">",
        ] {
            let _ = parse_org(s, PROD_ROOT);
        }
        for s in [
            "orgs-content<div class=\"box-content org ",
            "orgs-content<div class=\"box-content org main\"Organization rank</span>é",
            "orgs-content<div class=\"box-content org x\"class=\"ranking",
        ] {
            let _ = parse_citizen_orgs(s, PROD_ROOT);
        }
        for s in [
            "<li class=\"member-item",
            "<li class=\"member-item x\"class=\"stars\" style=\"width: 99999999999%",
            "<li class=\"member-item\"<ul class=\"rolelist\"><li class=\"role\">é",
            "<li class=\"member-item\"class=\"trans-03s name",
        ] {
            let env = json!({"success": 1, "data": {"totalrows": "7", "html": s}});
            let _ = parse_members(&env, 1, PROD_ROOT);
        }
    }

    /// TEST's profile and first member page, live.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_org() {
        let orgs = Orgs::new("sc-dossier-test");
        let o = orgs.org("TEST").await.unwrap();
        assert_eq!(o.sid, "TEST");
        assert!(o.members.unwrap() > 1000);
        let p = orgs.members("TEST", &MemberQuery::default()).await.unwrap();
        assert_eq!(p.members.len(), 32);
        assert!(p.total > 1000);
        assert!(orgs.org("ZZZNOPE999").await.is_err());
        let mine = orgs.citizen_orgs("VeeLume").await.unwrap();
        assert!(mine.iter().any(|m| m.sid.is_some() && m.rank.is_some()));
    }
}
