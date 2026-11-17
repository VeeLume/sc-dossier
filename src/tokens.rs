//! The player's mission token wallet (`MissionTokenService.QueryTokens`).
//!
//! Contract results write tokens; contract prerequisites read them. So the
//! wallet shows which intros, once-only contracts and chain steps the player has
//! done, plus running counters for tags that contracts award (event points,
//! for example). It is **not** a full contract history: plain repeatables leave
//! no token.
//!
//! Ids stay wire-native: [`TokenKind::Contract::name`] is a `ContractDefinition`
//! record name without its prefix, [`TokenKind::Tag::tag_id`] a `Tag` record
//! GUID. Naming them is the caller's job.

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::mission::MissionToken as ProtoToken;
use crate::proto::mission_token::mission_token_service_client::MissionTokenServiceClient;
use crate::proto::mission_token::QueryTokensRequest;

/// One token in the wallet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionToken {
    /// Token id: a UUID, the tag GUID for [`TokenKind::Tag`], or another
    /// server-chosen key (`Bounty_<geid>`).
    pub id: String,
    /// What the token records, parsed from [`MissionToken::reason`].
    pub kind: TokenKind,
    /// Count. For [`TokenKind::Tag`] the running total.
    pub count: i32,
    /// Creation, Unix milliseconds. For a tag token probably the first award.
    pub created_at_ms: u64,
    /// Expiry, Unix seconds; `None` when unset (all tokens seen so far).
    pub expires_at: Option<i64>,
    /// The raw reason string.
    pub reason: String,
}

/// What a token records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenKind {
    /// `Contract: <name>`: the contract was completed.
    Contract {
        /// `ContractDefinition` record name (`Hockrow_FacilityDelve_P1M2`).
        name: String,
    },
    /// `Contract: <name> Applied Tag: Tag.<guid> - Count: <n>`: a tag counter.
    Tag {
        /// `Tag` record GUID.
        tag_id: String,
        /// The contract that last added to the counter.
        contract: String,
        /// What that contract added.
        added: i32,
    },
    /// Anything else (location comms, `Bounty_<geid>` with no reason, …).
    Other,
}

const CONTRACT: &str = "Contract: ";
const APPLIED_TAG: &str = " Applied Tag: Tag.";
const COUNT: &str = " - Count: ";

impl TokenKind {
    /// Parse a token reason string.
    pub fn parse(reason: &str) -> Self {
        let Some(rest) = reason.strip_prefix(CONTRACT) else {
            return Self::Other;
        };
        match rest.split_once(APPLIED_TAG) {
            None if !rest.is_empty() && !rest.contains(' ') => Self::Contract {
                name: rest.to_string(),
            },
            None => Self::Other,
            Some((contract, tag)) => match tag.split_once(COUNT) {
                Some((tag_id, added)) => Self::Tag {
                    tag_id: tag_id.to_string(),
                    contract: contract.to_string(),
                    added: added.trim().parse().unwrap_or(0),
                },
                None => Self::Other,
            },
        }
    }
}

impl MissionToken {
    fn from_proto(t: ProtoToken) -> Self {
        Self {
            kind: TokenKind::parse(&t.reason),
            id: t.id,
            count: t.count,
            created_at_ms: t.created_at,
            expires_at: t.expiry.map(|e| e.seconds).filter(|&s| s > 0),
            reason: t.reason,
        }
    }
}

/// Every token of the player the JWT is scoped to. `jwt` must be the
/// player-scoped token.
pub(crate) async fn query(channel: &Channel, jwt: &str) -> Result<Vec<MissionToken>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = MissionTokenServiceClient::with_interceptor(channel.clone(), auth);
    let response = client.query_tokens(QueryTokensRequest {}).await?.into_inner();
    Ok(response.tokens.into_iter().map(MissionToken::from_proto).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contract() {
        assert_eq!(
            TokenKind::parse("Contract: Hockrow_FacilityDelve_P2M1-Stanton4"),
            TokenKind::Contract {
                name: "Hockrow_FacilityDelve_P2M1-Stanton4".into()
            }
        );
    }

    #[test]
    fn tag() {
        assert_eq!(
            TokenKind::parse(
                "Contract: Iasi_Courier_GoldItem_OnceOnly Applied Tag: \
                 Tag.d26f3e42-d9b4-41dc-a52b-174833256dac - Count: 250"
            ),
            TokenKind::Tag {
                tag_id: "d26f3e42-d9b4-41dc-a52b-174833256dac".into(),
                contract: "Iasi_Courier_GoldItem_OnceOnly".into(),
                added: 250,
            }
        );
    }

    #[test]
    fn other() {
        assert_eq!(TokenKind::parse(""), TokenKind::Other);
        assert_eq!(
            TokenKind::parse(
                "Onetime comms for entering location: StarMapObject.NyxSolarSystem"
            ),
            TokenKind::Other
        );
        assert_eq!(TokenKind::parse("Contract: "), TokenKind::Other);
    }
}
