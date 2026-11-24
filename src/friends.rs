//! The player's friend list with presence (`ContactsService.ListContacts`).
//!
//! A friend is online when [`Friend::presence`] is `Some`; the backend sends no
//! presence for offline friends. [`Activity`] says where: the main menu or the
//! persistent universe. Poll to follow changes: the push channel delivers no
//! presence updates (and the in-game list doesn't refresh either).

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::html::decode_entities;
use crate::proto::common_api::PaginationArguments;
use crate::proto::contacts::contacts_service_client::ContactsServiceClient;
use crate::proto::contacts::{Contact, ListContactsRequest, RelationshipType};
use crate::proto::presence::Status as ProtoStatus;

const PAGE_SIZE: u32 = 50;
const MAX_PAGES: usize = 100;

/// One friend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Friend {
    /// RSI handle.
    pub nickname: String,
    /// Display name (HTML entities decoded).
    pub display_name: String,
    /// RSI account id.
    pub account_id: u32,
    /// Player geid; `None` when the friend has no character (yet) in this
    /// environment.
    pub player_id: Option<u64>,
    /// When the friendship was recorded, Unix milliseconds. Friendships older
    /// than the contacts service share one migration time.
    pub since_ms: Option<i64>,
    /// `None` = offline.
    pub presence: Option<Presence>,
}

impl Friend {
    /// Whether the friend is online (in the menu or in the universe).
    pub fn is_online(&self) -> bool {
        self.presence.is_some()
    }
}

/// An online friend's presence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Presence {
    /// Status. The game sends [`Status::Activity`]; the others mirror Spectrum
    /// and have not been seen.
    pub status: Status,
    /// Where the friend is.
    pub activity: Activity,
    /// Free text from the activity; empty so far.
    pub details: String,
}

/// Presence status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Status {
    /// Online.
    Online,
    /// Away.
    Away,
    /// Do not disturb.
    DoNotDisturb,
    /// In the game; see [`Presence::activity`].
    Activity,
    /// Invisible.
    Invisible,
    /// A value this crate doesn't know.
    Unknown(i32),
}

/// Where an online friend is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Activity {
    /// Signed in, at the main menu (`menu`).
    Menu,
    /// In the persistent universe (`persistent_universe`).
    Universe,
    /// Another state string (Arena Commander, …) or none.
    Other(String),
}

impl Activity {
    fn parse(state: &str) -> Self {
        match state {
            "menu" => Self::Menu,
            "persistent_universe" => Self::Universe,
            s => Self::Other(s.to_string()),
        }
    }
}

impl Status {
    fn from_wire(v: i32) -> Self {
        match ProtoStatus::try_from(v) {
            Ok(ProtoStatus::Online) => Self::Online,
            Ok(ProtoStatus::Away) => Self::Away,
            Ok(ProtoStatus::Dnd) => Self::DoNotDisturb,
            Ok(ProtoStatus::Activity) => Self::Activity,
            Ok(ProtoStatus::Invisible) => Self::Invisible,
            _ => Self::Unknown(v),
        }
    }
}

/// Display names arrive HTML-escaped, sometimes twice (`&amp;lowbar;`).
fn unescape(s: &str) -> String {
    let once = decode_entities(s);
    if once.contains('&') {
        decode_entities(&once)
    } else {
        once
    }
}

impl Friend {
    fn from_proto(c: Contact) -> Option<Self> {
        let a = c.account?;
        let presence = c.presence.and_then(|p| {
            if matches!(
                ProtoStatus::try_from(p.status),
                Ok(ProtoStatus::Offline | ProtoStatus::Unspecified)
            ) {
                return None;
            }
            let (state, details) = p.activity.map(|a| (a.state, a.details)).unwrap_or_default();
            Some(Presence {
                status: Status::from_wire(p.status),
                activity: Activity::parse(&state),
                details,
            })
        });
        let since_ms = c
            .relationships
            .iter()
            .find(|r| r.r#type == RelationshipType::Friend as i32)
            .map(|r| r.created_at)
            .filter(|&t| t > 0);
        Some(Self {
            nickname: a.nickname,
            display_name: unescape(&a.display_name),
            account_id: a.account_id,
            player_id: (a.player_id != 0).then_some(a.player_id),
            since_ms,
            presence,
        })
    }
}

/// Every friend of the account the JWT belongs to. `jwt` must be the
/// player-scoped token.
pub(crate) async fn query(channel: &Channel, jwt: &str) -> Result<Vec<Friend>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ContactsServiceClient::with_interceptor(channel.clone(), auth);
    let mut out = Vec::new();
    let mut after = String::new();
    for _ in 0..MAX_PAGES {
        let page = client
            .list_contacts(ListContactsRequest {
                relationship_types: vec![RelationshipType::Friend as i32],
                target_account_ids: vec![],
                pagination: Some(PaginationArguments {
                    first: PAGE_SIZE,
                    after: after.clone(),
                }),
            })
            .await?
            .into_inner();
        out.extend(page.contacts.into_iter().filter_map(Friend::from_proto));
        match page.page_info {
            Some(p) if p.has_next_page && !p.end_cursor.is_empty() && p.end_cursor != after => {
                after = p.end_cursor;
            }
            _ => break,
        }
    }
    out.sort_by_key(|f| f.account_id);
    out.dedup_by_key(|f| f.account_id);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn double_escaped_name() {
        assert_eq!(unescape("Flux&amp;lowbar;Drifter"), "Flux_Drifter");
        assert_eq!(unescape("plain"), "plain");
    }

    #[test]
    fn activity() {
        assert_eq!(Activity::parse("menu"), Activity::Menu);
        assert_eq!(Activity::parse("persistent_universe"), Activity::Universe);
        assert_eq!(Activity::parse("arena"), Activity::Other("arena".into()));
    }
}
