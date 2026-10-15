//! Spectrum — RSI's chat/forum platform — over its REST API, on the launcher
//! session (or anonymously).
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! use sc_dossier::spectrum::{SearchQuery, Spectrum};
//! let spectrum = Spectrum::from_launcher("my-app/1.0")?;
//! let page = spectrum.search(&SearchQuery::staff()).await?;
//! for post in &page.posts {
//!     println!("{} {}: {}", post.time_created, post.member.nickname, post.body);
//! }
//! # Ok(()) }
//! ```
//!
//! Every route is a JSON `POST` under `<root>/api/spectrum`, answered with
//! `{success, code, msg, data}`; [`Spectrum::call`] unwraps it.
//!
//! **Rate limit**: per IP and per route, ~1 request/second; past it the server
//! answers [`Error::Throttled`] (HTTP 200, code `ErrThrottleLimit` — not a 429)
//! and clears within ~2 minutes. Pacing is the caller's job.
//!
//! **Visibility**: results honour lobby permissions. Anonymous clients don't
//! see restricted lobbies (e.g. SC's `sc-testing-chat`, `concierge`), and lobby
//! history needs a session even for public lobbies.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{Error, Result};
use crate::store::{self, Credentials};

/// Production host (`rootUrl` in the launcher).
pub const PROD_ROOT: &str = "https://robertsspaceindustries.com";
/// The Star Citizen community id.
pub const SC_COMMUNITY: &str = "1";
/// Community role id for CIG staff in [`SC_COMMUNITY`]; staff posts carry it as
/// their `highlight_role_id`. Role ids are per community — see
/// [`Spectrum::community_roles`].
pub const STAFF_ROLE: u32 = 2;

/// A Spectrum REST client.
#[derive(Clone)]
pub struct Spectrum {
    http: reqwest::Client,
    base: String,
    root: String,
    user_agent: String,
    /// `(X-Rsi-Token, X-Rsi-Device)`; `None` = anonymous.
    creds: Option<(String, String)>,
}

impl std::fmt::Debug for Spectrum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spectrum")
            .field("root", &self.root)
            .field("authenticated", &self.creds.is_some())
            .finish()
    }
}

impl Spectrum {
    /// Authenticated with the RSI launcher's stored session.
    pub fn from_launcher(user_agent: &str) -> Result<Self> {
        Ok(Self::from_credentials(
            &store::read_credentials()?,
            user_agent,
        ))
    }

    /// Authenticated with already-read launcher [`Credentials`].
    pub fn from_credentials(creds: &Credentials, user_agent: &str) -> Self {
        Self::new(
            user_agent,
            Some((creds.session_token.clone(), creds.device_token.clone())),
        )
    }

    /// Without credentials — what a logged-out browser sees.
    pub fn anonymous(user_agent: &str) -> Self {
        Self::new(user_agent, None)
    }

    fn new(user_agent: &str, creds: Option<(String, String)>) -> Self {
        let mut s = Self {
            http: reqwest::Client::new(),
            base: String::new(),
            root: String::new(),
            user_agent: user_agent.to_string(),
            creds,
        };
        s.set_root(PROD_ROOT);
        s
    }

    /// Use another host (PTU, staging). Defaults to [`PROD_ROOT`].
    pub fn with_root(mut self, root: &str) -> Self {
        self.set_root(root);
        self
    }

    fn set_root(&mut self, root: &str) {
        self.root = root.trim_end_matches('/').to_string();
        self.base = format!("{}/api/spectrum", self.root);
    }

    /// Whether requests carry the launcher session.
    pub fn is_authenticated(&self) -> bool {
        self.creds.is_some()
    }

    /// POST `path` (e.g. `"/search/content/extended"`) and return the
    /// envelope's `data`. `body: None` sends an empty body. Escape hatch for
    /// routes without a typed wrapper.
    pub async fn call(&self, path: &str, body: Option<&Value>) -> Result<Value> {
        let mut req = self
            .http
            .post(format!("{}{path}", self.base))
            .header("User-Agent", &self.user_agent)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json");
        if let Some((token, device)) = &self.creds {
            req = req
                .header("X-Rsi-Token", token)
                .header("X-Rsi-Device", device);
        }
        req = match body {
            Some(b) => req.json(b),
            // reqwest omits Content-Length on an empty body; the server then
            // answers 411.
            None => req.header("Content-Length", "0"),
        };

        let env: Value = req.send().await?.json().await?;
        if env.get("success").and_then(Value::as_i64) == Some(1) {
            return Ok(env.get("data").cloned().unwrap_or(Value::Null));
        }
        let code = env.get("code").and_then(Value::as_str).map(str::to_string);
        if code.as_deref() == Some("ErrThrottleLimit") {
            return Err(Error::Throttled {
                path: path.to_string(),
            });
        }
        Err(Error::Spectrum {
            path: path.to_string(),
            code,
            msg: env.get("msg").and_then(Value::as_str).map(str::to_string),
        })
    }

    /// One page (25 hits) of content search, newest first unless
    /// [`Sort::Relevance`].
    pub async fn search(&self, query: &SearchQuery) -> Result<SearchPage> {
        let data = self
            .call("/search/content/extended", Some(&query.to_json()))
            .await?;
        let hits = data.pointer("/hits/hits").and_then(Value::as_array);
        Ok(SearchPage {
            total: data
                .pointer("/hits/total")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            posts: hits
                .into_iter()
                .flatten()
                .filter_map(|h| Post::from_hit(h, &self.root))
                .collect(),
        })
    }

    /// Resolve a nickname (e.g. `"Wakapedia-CIG"`) to its member id, the value
    /// [`SearchQuery::author`] takes.
    pub async fn member_id(&self, nickname: &str) -> Result<String> {
        let data = self
            .call(
                "/member/info/nickname",
                Some(&json!({ "nickname": nickname })),
            )
            .await?;
        id(&data, "/member/id")
            .ok_or_else(|| Error::Invalid(format!("no member id for nickname {nickname}")))
    }

    /// A community's roles (id → name), e.g. `2` = staff in [`SC_COMMUNITY`].
    pub async fn community_roles(&self, community_id: &str) -> Result<Vec<Role>> {
        let data = self
            .call(
                "/v2/community/roles",
                Some(&json!({ "community_id": community_id })),
            )
            .await?;
        Ok(data
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|r| {
                Some(Role {
                    id: id(r, "/id")?,
                    name: r.get("name")?.as_str()?.to_string(),
                })
            })
            .collect())
    }

    /// One forum reply by id (`/v2/forum/thread/reply/get`) — e.g. a search
    /// hit's [`Post::id`], then its [`Reply::parent_reply_id`] for what it
    /// answers. Works anonymously.
    pub async fn reply(&self, reply_id: &str) -> Result<Reply> {
        let data = self
            .call(
                "/v2/forum/thread/reply/get",
                Some(&json!({ "reply_id": reply_id })),
            )
            .await?;
        Reply::from_value(&data)
            .ok_or_else(|| Error::Invalid(format!("reply {reply_id}: unexpected shape")))
    }

    /// A forum thread's opening post by its slug ([`Thread::slug`]) — what a
    /// top-level reply answers (`/v2/forum/thread/get`). Works anonymously.
    pub async fn thread_opener(&self, slug: &str) -> Result<ThreadOpener> {
        let data = self
            .call("/v2/forum/thread/get", Some(&json!({ "slug": slug })))
            .await?;
        ThreadOpener::from_value(&data)
            .ok_or_else(|| Error::Invalid(format!("thread {slug}: unexpected shape")))
    }

    /// Up to 50 lobby messages, newest page first; page back by passing the
    /// oldest returned id as `before`. `before` is inclusive: a search hit's
    /// [`Post::id`] returns that message and the 49 before it. Needs a session
    /// even for public lobbies.
    pub async fn lobby_history(
        &self,
        lobby_id: &str,
        before: Option<&str>,
    ) -> Result<Vec<Message>> {
        let body = json!({
            "lobby_id": lobby_id,
            "timeframe": "before",
            "message_id": before,
            "size": 50,
        });
        let data = self.call("/message/history", Some(&body)).await?;
        Ok(data
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Message::from_value)
            .collect())
    }
}

/// Which content a search covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// Forum thread openers (`op`).
    Thread,
    /// Forum replies (`reply`).
    Reply,
    /// Chat lobby messages (`chat`).
    Chat,
}

/// Search result order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sort {
    /// Newest first.
    Latest,
    /// Best match first.
    Relevance,
}

/// Search time window, counted back from now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// Last day.
    Day,
    /// Last week.
    Week,
    /// Last month — **broken server-side** (returns a handful of hits where
    /// `Week` returns hundreds). Use [`Range::Year`] and cut off client-side.
    Month,
    /// Last year.
    Year,
    /// No limit.
    All,
}

/// A content search. [`SearchQuery::staff`] is the CIG staff feed.
#[derive(Debug, Clone)]
pub struct SearchQuery {
    /// Community id.
    pub community_id: String,
    /// Content kinds to include.
    pub scopes: Vec<Scope>,
    /// Free text; empty matches everything.
    pub text: String,
    /// 1-based page (25 hits each).
    pub page: u32,
    /// Result order.
    pub sort: Sort,
    /// Time window.
    pub range: Range,
    /// Only posts by this member id ([`Spectrum::member_id`]).
    pub author: Option<String>,
    /// Only posts highlighted with this role id ([`STAFF_ROLE`]).
    pub highlight_role_id: Option<u32>,
}

impl SearchQuery {
    /// Everything in `community_id`: all scopes, newest first, last year, page 1.
    pub fn new(community_id: &str) -> Self {
        Self {
            community_id: community_id.to_string(),
            scopes: vec![Scope::Thread, Scope::Reply, Scope::Chat],
            text: String::new(),
            page: 1,
            sort: Sort::Latest,
            range: Range::Year,
            author: None,
            highlight_role_id: None,
        }
    }

    /// Staff-highlighted posts in [`SC_COMMUNITY`] — forum and chat.
    pub fn staff() -> Self {
        Self::new(SC_COMMUNITY).highlight_role(STAFF_ROLE)
    }

    /// Restrict to one author (member id).
    pub fn author(mut self, member_id: &str) -> Self {
        self.author = Some(member_id.to_string());
        self
    }

    /// Restrict to posts highlighted with `role_id`.
    pub fn highlight_role(mut self, role_id: u32) -> Self {
        self.highlight_role_id = Some(role_id);
        self
    }

    /// Set the content kinds.
    pub fn scopes(mut self, scopes: &[Scope]) -> Self {
        self.scopes = scopes.to_vec();
        self
    }

    /// Set the free-text query.
    pub fn text(mut self, text: &str) -> Self {
        self.text = text.to_string();
        self
    }

    /// Set the time window.
    pub fn range(mut self, range: Range) -> Self {
        self.range = range;
        self
    }

    /// Set the 1-based page.
    pub fn page(mut self, page: u32) -> Self {
        self.page = page;
        self
    }

    /// Set the order.
    pub fn sort(mut self, sort: Sort) -> Self {
        self.sort = sort;
        self
    }

    fn to_json(&self) -> Value {
        let scopes: Vec<&str> = self
            .scopes
            .iter()
            .map(|s| match s {
                Scope::Thread => "op",
                Scope::Reply => "reply",
                Scope::Chat => "chat",
            })
            .collect();
        let mut v = json!({
            "community_id": self.community_id,
            "type": scopes,
            "text": self.text,
            "page": self.page,
            "sort": match self.sort { Sort::Latest => "latest", Sort::Relevance => "score" },
            "range": match self.range {
                Range::Day => "day",
                Range::Week => "week",
                Range::Month => "month",
                Range::Year => "year",
                Range::All => "all",
            },
            "visibility": "nonerased",
        });
        if let Some(a) = &self.author {
            v["author"] = json!(a);
        }
        if let Some(r) = self.highlight_role_id {
            v["highlight_role_id"] = json!(r);
        }
        v
    }
}

/// One page of search results.
#[derive(Debug, Clone)]
pub struct SearchPage {
    /// Total matches (the server caps this at 10 000).
    pub total: u64,
    /// This page's posts.
    pub posts: Vec<Post>,
}

/// What a [`Post`] is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PostKind {
    /// A chat lobby message.
    Chat,
    /// A forum thread opener.
    Thread,
    /// A forum reply.
    Reply,
}

/// A search hit.
#[derive(Debug, Clone)]
pub struct Post {
    /// Message / thread / reply id (unique within its kind).
    pub id: String,
    /// What this is.
    pub kind: PostKind,
    /// Community id.
    pub community_id: String,
    /// Community slug (e.g. `"SC"`), used in URLs.
    pub community_slug: String,
    /// Author.
    pub member: Member,
    /// Text as delivered: plain text with HTML entities escaped (`&#039;`).
    pub body: String,
    /// Role the post is highlighted with ([`STAFF_ROLE`] for staff).
    pub highlight_role_id: Option<u32>,
    /// Creation time, ISO 8601 UTC.
    pub time_created: String,
    /// Creation time, Unix milliseconds (the search sort key).
    pub time_ms: i64,
    /// The chat lobby, for [`PostKind::Chat`].
    pub lobby: Option<Place>,
    /// The forum channel, for threads and replies.
    pub channel: Option<Place>,
    /// The forum thread, for threads and replies.
    pub thread: Option<Thread>,
    /// Link to the post on the website.
    pub url: String,
}

/// A lobby or forum channel.
#[derive(Debug, Clone)]
pub struct Place {
    /// Id.
    pub id: String,
    /// Display name.
    pub name: String,
}

/// A forum thread.
#[derive(Debug, Clone)]
pub struct Thread {
    /// Thread id.
    pub id: String,
    /// URL slug.
    pub slug: String,
    /// Title.
    pub subject: String,
    /// Id of the opener's content reply — what RSI's dev tracker links to.
    pub content_reply_id: Option<String>,
}

/// A Spectrum member.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Member id.
    pub id: String,
    /// Handle, e.g. `"Wakapedia-CIG"`.
    pub nickname: String,
    /// Display name.
    pub displayname: String,
    /// The `isGM` flag.
    pub is_gm: bool,
    /// Role ids per community id. Filled by [`Spectrum::lobby_history`]; empty
    /// on search hits.
    pub roles: BTreeMap<String, Vec<String>>,
}

/// A community role.
#[derive(Debug, Clone)]
pub struct Role {
    /// Role id.
    pub id: String,
    /// Name, e.g. `"staff"`.
    pub name: String,
}

/// A chat lobby message from [`Spectrum::lobby_history`].
///
/// Chat has no reply-to relation; [`Message::mentions`] is the only link to
/// another member.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Message {
    /// Message id.
    pub id: String,
    /// Lobby id.
    pub lobby_id: String,
    /// Author, with roles.
    pub member: Member,
    /// Plain text: the message's rich-text blocks joined by newlines.
    pub text: String,
    /// Members @-mentioned, in entity order.
    pub mentions: Vec<Mention>,
    /// Role the message is highlighted with.
    pub highlight_role_id: Option<u32>,
    /// Creation time, Unix seconds.
    pub time_created: i64,
}

/// An @-mention in a chat message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    /// Member id.
    pub member_id: String,
    /// Nickname, as [`Member::nickname`] has it.
    pub nickname: String,
}

/// A forum reply from [`Spectrum::reply`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Reply {
    /// Reply id.
    pub id: String,
    /// Thread id.
    pub thread_id: String,
    /// The reply this one answers; `None` for a top-level reply, which
    /// answers the thread opener ([`Spectrum::thread_opener`]). Nesting can be
    /// deep, and the parent can itself be a staff reply.
    pub parent_reply_id: Option<String>,
    /// The top-level reply of this branch (itself when top-level).
    pub root_reply_id: Option<String>,
    /// Author.
    pub member: Member,
    /// Plain text (Spectrum's `plaintext`; empty when erased).
    pub text: String,
    /// Role the reply is highlighted with.
    pub highlight_role_id: Option<u32>,
    /// Whether it was deleted.
    pub is_erased: bool,
    /// Creation time, Unix milliseconds.
    pub time_ms: i64,
}

/// A forum thread's opening post from [`Spectrum::thread_opener`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ThreadOpener {
    /// Thread id.
    pub thread_id: String,
    /// URL slug.
    pub slug: String,
    /// Title.
    pub subject: String,
    /// Author.
    pub member: Member,
    /// Plain text.
    pub text: String,
    /// Role the opener is highlighted with.
    pub highlight_role_id: Option<u32>,
    /// Creation time, Unix milliseconds.
    pub time_ms: i64,
}

impl Post {
    fn from_hit(h: &Value, root: &str) -> Option<Self> {
        let kind = match h.get("_index")?.as_str()? {
            "tavern_message" => PostKind::Chat,
            "tavern_forum_thread_op" => PostKind::Thread,
            "tavern_forum_thread_reply" => PostKind::Reply,
            _ => return None,
        };
        let src = h.get("_source")?;
        let d = h.get("details").unwrap_or(&Value::Null);
        let place = |key: &str| {
            Some(Place {
                id: id(d, &format!("/{key}/id"))?,
                name: d.pointer(&format!("/{key}/name"))?.as_str()?.to_string(),
            })
        };
        let thread = d.get("thread").and_then(|t| {
            Some(Thread {
                id: id(t, "/id")?,
                slug: t.get("slug")?.as_str()?.to_string(),
                subject: t
                    .get("subject")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
                content_reply_id: id(t, "/content_reply_id"),
            })
        });
        let id_ = id(h, "/_id")?;
        let community_slug = d
            .pointer("/community/slug")
            .and_then(Value::as_str)
            .unwrap_or("SC")
            .to_string();
        let lobby = place("lobby");
        let channel = place("channel");

        let base = format!("{root}/spectrum/community/{community_slug}");
        let url = match (kind, &lobby, &channel, &thread) {
            (PostKind::Chat, Some(l), _, _) => format!("{base}/lobby/{}/message/{id_}", l.id),
            (PostKind::Thread, _, Some(c), Some(t)) => {
                format!("{base}/forum/{}/thread/{}", c.id, t.slug)
            }
            (PostKind::Reply, _, Some(c), Some(t)) => {
                format!("{base}/forum/{}/thread/{}/{id_}", c.id, t.slug)
            }
            _ => base,
        };

        Some(Self {
            id: id_,
            kind,
            community_id: id(src, "/community_id").unwrap_or_default(),
            community_slug,
            member: Member::from_value(d.get("member")?)?,
            body: src
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            highlight_role_id: id(src, "/highlight_role_id").and_then(|s| s.parse().ok()),
            time_created: src
                .get("time_created")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            time_ms: h.pointer("/sort/0").and_then(Value::as_i64).unwrap_or(0),
            lobby,
            channel,
            thread,
            url,
        })
    }
}

impl Member {
    fn from_value(m: &Value) -> Option<Self> {
        let roles = m
            .get("roles")
            .and_then(Value::as_object)
            .map(|o| {
                o.iter()
                    .map(|(c, ids)| {
                        let ids = ids.as_array().into_iter().flatten();
                        (c.clone(), ids.filter_map(|v| id(v, "")).collect())
                    })
                    .collect()
            })
            .unwrap_or_default();
        Some(Self {
            id: id(m, "/id")?,
            nickname: m.get("nickname")?.as_str()?.to_string(),
            displayname: m
                .get("displayname")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            // Chat and search send `isGM`, the v2 forum API `isGm`.
            is_gm: m
                .get("isGM")
                .or_else(|| m.get("isGm"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
            roles,
        })
    }
}

impl Message {
    fn from_value(m: &Value) -> Option<Self> {
        let blocks = m.pointer("/content_state/blocks").and_then(Value::as_array);
        let text = blocks
            .into_iter()
            .flatten()
            .filter_map(|b| b.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        // Draft.js `entityMap`: an object keyed by index, or (as served) an array.
        let entities: Vec<&Value> = match m.pointer("/content_state/entityMap") {
            Some(Value::Array(a)) => a.iter().collect(),
            Some(Value::Object(o)) => o.values().collect(),
            _ => Vec::new(),
        };
        let mentions = entities
            .into_iter()
            .filter(|e| e.get("type").and_then(Value::as_str) == Some("MENTION"))
            .filter_map(|e| {
                Some(Mention {
                    member_id: id(e, "/data/id")?,
                    nickname: e.pointer("/data/nickname")?.as_str()?.to_string(),
                })
            })
            .collect();
        Some(Self {
            id: id(m, "/id")?,
            lobby_id: id(m, "/lobby_id").unwrap_or_default(),
            member: Member::from_value(m.get("member")?)?,
            text,
            mentions,
            highlight_role_id: id(m, "/highlight_role_id").and_then(|s| s.parse().ok()),
            time_created: m.get("time_created").and_then(Value::as_i64).unwrap_or(0),
        })
    }
}

impl Reply {
    fn from_value(r: &Value) -> Option<Self> {
        Some(Self {
            id: id(r, "/id")?,
            thread_id: id(r, "/threadId")?,
            parent_reply_id: id(r, "/parentReplyId"),
            root_reply_id: id(r, "/rootReplyId"),
            member: Member::from_value(r.get("author")?)?,
            text: plaintext(r),
            highlight_role_id: id(r, "/highlightRoleId").and_then(|s| s.parse().ok()),
            is_erased: r.get("isErased").and_then(Value::as_bool).unwrap_or(false),
            time_ms: created_ms(r),
        })
    }
}

impl ThreadOpener {
    fn from_value(t: &Value) -> Option<Self> {
        let s = |k: &str| t.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        Some(Self {
            thread_id: id(t, "/id")?,
            slug: s("slug"),
            subject: s("subject"),
            member: Member::from_value(t.get("member")?)?,
            text: plaintext(t),
            highlight_role_id: id(t, "/highlightRoleId").and_then(|s| s.parse().ok()),
            time_ms: created_ms(t),
        })
    }
}

fn plaintext(v: &Value) -> String {
    v.get("plaintext")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

/// v2 forum `createdAt` is Unix seconds.
fn created_ms(v: &Value) -> i64 {
    v.get("createdAt")
        .and_then(Value::as_i64)
        .map_or(0, |s| s.saturating_mul(1000))
}

/// Spectrum sends ids as strings or numbers, inconsistently; read either.
fn id(v: &Value, pointer: &str) -> Option<String> {
    match v.pointer(pointer)? {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_json_matches_the_web_client() {
        let q = SearchQuery::staff()
            .author("429390")
            .range(Range::Week)
            .page(2);
        let v = q.to_json();
        assert_eq!(v["type"], json!(["op", "reply", "chat"]));
        assert_eq!(v["highlight_role_id"], json!(2));
        assert_eq!(v["author"], json!("429390"));
        assert_eq!(v["range"], json!("week"));
        assert_eq!(v["sort"], json!("latest"));
        assert_eq!(v["visibility"], json!("nonerased"));
    }

    #[test]
    fn parses_hits_of_each_kind() {
        let member = json!({"id": "1", "nickname": "Dev-CIG", "displayname": "Dev", "isGM": true});
        let chat = json!({
            "_index": "tavern_message", "_id": "67272564", "sort": [1791857751000i64],
            "_source": {"body": "it&#039;s up", "community_id": "1", "highlight_role_id": "2",
                        "time_created": "2026-10-13T02:15:51Z"},
            "details": {"community": {"slug": "SC"}, "lobby": {"id": "38230", "name": "sc-testing-chat"},
                        "member": member}
        });
        let reply = json!({
            "_index": "tavern_forum_thread_reply", "_id": "9176513", "sort": [1i64],
            "_source": {"body": "b", "community_id": "1", "highlight_role_id": 2},
            "details": {"community": {"slug": "SC"}, "channel": {"id": "3", "name": "General"},
                        "thread": {"id": "549514", "slug": "vr-support", "subject": "VR",
                                   "content_reply_id": "8839503"},
                        "member": member}
        });
        let c = Post::from_hit(&chat, PROD_ROOT).unwrap();
        assert_eq!(c.kind, PostKind::Chat);
        assert_eq!(c.highlight_role_id, Some(2));
        assert_eq!(c.time_ms, 1791857751000);
        assert!(c
            .url
            .ends_with("/spectrum/community/SC/lobby/38230/message/67272564"));
        let r = Post::from_hit(&reply, PROD_ROOT).unwrap();
        assert_eq!(r.kind, PostKind::Reply);
        assert_eq!(r.highlight_role_id, Some(2));
        assert_eq!(
            r.thread.unwrap().content_reply_id.as_deref(),
            Some("8839503")
        );
        assert!(r.url.ends_with("/forum/3/thread/vr-support/9176513"));
    }

    #[test]
    fn parses_history_message_with_roles() {
        let m = json!({
            "id": "67272165", "lobby_id": "38230", "highlight_role_id": null, "time_created": 1791847789,
            "member": {"id": "419208", "nickname": "Nightrider-CIG", "isGM": true,
                       "roles": {"1": ["11", "2", "3", "4"]}},
            "content_state": {"blocks": [{"text": "line one"}, {"text": "line two"}]}
        });
        let msg = Message::from_value(&m).unwrap();
        assert_eq!(msg.text, "line one\nline two");
        assert_eq!(msg.member.roles["1"], vec!["11", "2", "3", "4"]);
        assert_eq!(msg.highlight_role_id, None);
        assert!(msg.mentions.is_empty());
    }

    fn fixture(name: &str) -> Value {
        let path = format!(
            "{}/tests/fixtures/spectrum/{name}",
            env!("CARGO_MANIFEST_DIR")
        );
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn nested_reply_points_at_its_parent() {
        let r = Reply::from_value(&fixture("reply_nested.json")).unwrap();
        assert_eq!(r.id, "9168775");
        assert_eq!(r.thread_id, "572419");
        assert_eq!(r.parent_reply_id.as_deref(), Some("9168717"));
        assert_eq!(r.root_reply_id.as_deref(), Some("9164138"));
        assert_eq!(r.member.nickname, "mkale-CIG");
        assert_eq!(r.highlight_role_id, Some(STAFF_ROLE));
        assert!(r.text.starts_with("Thanks for the detailed write-up"));
        assert!(r.time_ms > 1_700_000_000_000);
        // Its parent is itself nested.
        let p = Reply::from_value(&fixture("reply_parent.json")).unwrap();
        assert_eq!(p.id, "9168717");
        assert_eq!(p.parent_reply_id.as_deref(), Some("9168106"));
        assert_eq!(p.text, "Player question, first line.\nSecond line.");
        assert!(!p.is_erased);
    }

    #[test]
    fn top_level_reply_answers_the_opener() {
        let r = Reply::from_value(&fixture("reply_top_level.json")).unwrap();
        assert_eq!(r.parent_reply_id, None);
        assert_eq!(r.root_reply_id.as_deref(), Some(r.id.as_str()));
        assert_eq!(r.member.nickname, "CIG-SeanT");
        let o = ThreadOpener::from_value(&fixture("thread_opener.json")).unwrap();
        assert_eq!(o.thread_id, r.thread_id);
        assert_eq!(o.slug, "can-we-please-get-some-new-emotes-poses-and-or-idl");
        assert!(o.subject.starts_with("Can we please get some new emotes"));
        assert_eq!(o.member.nickname, "Player12");
        assert!(!o.text.is_empty());
        assert!(o.time_ms > 0);
    }

    #[test]
    fn chat_mentions() {
        let m = Message::from_value(&fixture("chat_mention.json")).unwrap();
        assert_eq!(m.member.nickname, "Nightrider-CIG");
        assert_eq!(
            m.mentions,
            [Mention {
                member_id: "900001".into(),
                nickname: "Player1".into()
            }]
        );
        // Draft.js's object-keyed entityMap form.
        let keyed = json!({"id": "1", "member": {"id": "2", "nickname": "A"},
            "content_state": {"blocks": [], "entityMap": {"0": {"type": "MENTION",
                "data": {"id": 7, "nickname": "B"}}, "1": {"type": "EMOJI", "data": ":x:"}}}});
        let k = Message::from_value(&keyed).unwrap();
        assert_eq!(k.mentions.len(), 1);
        assert_eq!(k.mentions[0].member_id, "7");
    }

    #[test]
    fn reply_shapes_that_do_not_parse() {
        assert!(Reply::from_value(&json!({})).is_none());
        assert!(Reply::from_value(&json!({"id": 1, "threadId": 2})).is_none());
        assert!(ThreadOpener::from_value(&json!({"id": 1, "member": null})).is_none());
    }

    /// Newest staff forum reply, live: resolve what it answers.
    #[tokio::test]
    #[ignore = "live: hits robertsspaceindustries.com"]
    async fn live_resolves_a_staff_reply_parent() {
        let sp = Spectrum::anonymous("sc-dossier-test");
        let page = sp
            .search(&SearchQuery::staff().scopes(&[Scope::Reply]))
            .await
            .unwrap();
        let hit = page.posts.first().expect("a staff reply");
        let pace = || tokio::time::sleep(std::time::Duration::from_millis(1500));
        pace().await;
        let r = sp.reply(&hit.id).await.unwrap();
        assert_eq!(r.id, hit.id);
        assert_eq!(Some(&r.thread_id), hit.thread.as_ref().map(|t| &t.id));
        pace().await;
        match &r.parent_reply_id {
            Some(parent) => {
                let p = sp.reply(parent).await.unwrap();
                assert_eq!(p.thread_id, r.thread_id);
                assert!(!p.text.is_empty() || p.is_erased);
            }
            None => {
                let o = sp
                    .thread_opener(&hit.thread.as_ref().unwrap().slug)
                    .await
                    .unwrap();
                assert_eq!(o.thread_id, r.thread_id);
            }
        }
    }
}
