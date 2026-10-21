//! The player's reputation standings and the public [`Reputation`] type.
//!
//! A standing is keyed by `(org, scope)`: one org can hold several, e.g.
//! Covalex `Hauling` and `FactionReputation`. Ids stay wire-native, and every
//! one is a DCB key:
//!
//! - [`Reputation::org_geid`] = `FactionReputation.GEID` (the org's name).
//! - [`Reputation::scope_id`] = an `SReputationScopeParams` record GUID, which
//!   holds the full rank ladder (ceiling, every rank). The backend returns only
//!   the current rank.
//! - [`Standing::id`] = an `SReputationStandingParams` record GUID (the rank's
//!   display name).
//!
//! Resolving them is the caller's job (sc-holotable).

use std::collections::HashSet;

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::common_api::{PaginationArguments, Query};
use crate::proto::reputation::reputation_service_client::ReputationServiceClient;
use crate::proto::reputation::{QueryReputationsRequest, VersionedReputation};

const PAGE_SIZE: u32 = 100;

const ORG_URN: &str = "urn:sc:reputations:org:geid:";

/// One reputation standing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reputation {
    /// Row id (UUID).
    pub id: String,
    /// The org's GEID, from [`Reputation::entity`]; `None` if the urn has
    /// another shape.
    pub org_geid: Option<u64>,
    /// Raw entity urn (`urn:sc:reputations:org:geid:199998949491`).
    pub entity: String,
    /// Scope name (`"FactionReputation"`, `"Hauling"`, `"Security"`,
    /// `"MissionProviderReputation_Battaglia"`, …).
    pub scope: String,
    /// Current score.
    pub score: i32,
    /// The scope's rank table (`standings_id`): an `SReputationScopeParams`
    /// GUID. One per scope, shared by every org in it.
    pub scope_id: String,
    /// The rank the score currently falls in.
    pub standing: Option<Standing>,
    /// Last change, Unix milliseconds.
    pub updated_at_ms: u64,
}

/// A rank: the band [`Reputation::score`] falls in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Standing {
    /// `SReputationStandingParams` GUID.
    pub id: String,
    /// Raw rank name (`"FactionRep_Allied_Rank1"`); the display name is in the
    /// DCB record.
    pub name: String,
    /// Lowest score in this rank.
    pub min: i32,
    /// Score drifted per [`Standing::drift_every_secs`]; `0` = no drift.
    pub drift_amount: i32,
    /// Drift interval in seconds (DCB `driftTimeHours` × 3600).
    pub drift_every_secs: u32,
    /// The DCB `gated` flag. Set on entry ranks; what it gates is unknown.
    pub gated: bool,
}

impl Reputation {
    fn from_proto(v: &VersionedReputation) -> Option<Self> {
        let r = v.reputation.as_ref()?;
        Some(Self {
            id: r.id.clone(),
            org_geid: org_geid(&r.entity),
            entity: r.entity.clone(),
            scope: r.scope.clone(),
            score: r.score,
            scope_id: r.standings_id.clone(),
            standing: v.standing.as_ref().map(|s| Standing {
                id: s.id.clone(),
                name: s.name.clone(),
                min: s.min,
                drift_amount: s.drift.as_ref().map_or(0, |d| d.amount),
                drift_every_secs: s.drift.as_ref().map_or(0, |d| d.time),
                gated: s.gated,
            }),
            updated_at_ms: r.updated_at,
        })
    }
}

/// `urn:sc:reputations:org:geid:<u64>` → the GEID.
fn org_geid(urn: &str) -> Option<u64> {
    urn.strip_prefix(ORG_URN)?.parse().ok()
}

/// Every standing of the player the JWT is scoped to; an empty query is
/// scoped to the caller. `jwt` must be the player-scoped token.
pub(crate) async fn query(channel: &Channel, jwt: &str) -> Result<Vec<Reputation>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ReputationServiceClient::with_interceptor(channel.clone(), auth);

    let mut after = String::new();
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let request = QueryReputationsRequest {
            query: Some(Query {
                filter: None,
                pagination: Some(PaginationArguments {
                    first: PAGE_SIZE,
                    after: after.clone(),
                }),
                sort: None,
            }),
        };
        let response = client.query_reputations(request).await?.into_inner();
        for r in response.results.iter().filter_map(Reputation::from_proto) {
            if seen.insert(r.id.clone()) {
                out.push(r);
            }
        }
        match response.page_info {
            Some(info) if info.has_next_page && !info.end_cursor.is_empty() => {
                after = info.end_cursor;
            }
            _ => break,
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::reputation::{Drift, Reputation as ProtoRep, Standing as ProtoStanding};

    #[test]
    fn org_urns() {
        assert_eq!(
            org_geid("urn:sc:reputations:org:geid:199998949491"),
            Some(199998949491)
        );
        assert_eq!(org_geid("urn:sc:global:player:geid:1"), None);
        assert_eq!(org_geid("urn:sc:reputations:org:geid:"), None);
        assert_eq!(org_geid("urn:sc:reputations:org:geid:-5"), None);
        assert_eq!(org_geid(""), None);
    }

    /// A row from the live run (4.10.0), as decoded.
    #[test]
    fn from_proto() {
        let v = VersionedReputation {
            reputation: Some(ProtoRep {
                id: "e61641fe-2e08-5f4d-ba50-851dffcda367".into(),
                standings_id: "d31bd373-35ab-4ea4-b0a7-0ccc867ef082".into(),
                updated_at: 1784873655353,
                subject: "urn:sc:global:player:geid:204717100112".into(),
                entity: "urn:sc:reputations:org:geid:199998982640".into(),
                scope: "FactionReputation".into(),
                score: 120,
                last_drifted_at: 1784873655319,
                lock: None,
            }),
            standing: Some(ProtoStanding {
                id: "a8f635c5-7fc0-4d27-8a08-27bb3476bfb8".into(),
                name: "FactionRep_Neutral_Rank0".into(),
                description: "desc".into(),
                min: 0,
                drift: Some(Drift {
                    time: 86400,
                    amount: 0,
                }),
                gated: true,
            }),
            version: 0,
        };
        let r = Reputation::from_proto(&v).unwrap();
        assert_eq!(r.org_geid, Some(199998982640));
        assert_eq!(
            (r.scope.as_str(), r.score, r.scope_id.as_str()),
            (
                "FactionReputation",
                120,
                "d31bd373-35ab-4ea4-b0a7-0ccc867ef082"
            )
        );
        let s = r.standing.unwrap();
        assert_eq!(
            (s.name.as_str(), s.min, s.drift_every_secs, s.gated),
            ("FactionRep_Neutral_Rank0", 0, 86400, true)
        );
        assert!(Reputation::from_proto(&VersionedReputation::default()).is_none());
    }
}
