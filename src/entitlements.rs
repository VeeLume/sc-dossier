//! Entitlement-ledger query and the public [`Entitlement`] type — every
//! owned-item grant (web hangar + in-game). Returns raw IDs; GUID→name
//! resolution is the consumer's responsibility.

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::entitlement::external_entitlement_service_client::ExternalEntitlementServiceClient;
use crate::proto::entitlement::{Entitlement as ProtoEntitlement, QueryEntitlementsStreamRequest};

/// Where an entitlement came from. `Unknown` covers `UNSPECIFIED` and any
/// future value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementSource {
    /// `..._PLATFORM` (1) — webstore / web hangar.
    Platform,
    /// `..._INGAME_PERSISTENT` (2).
    IngamePersistent,
    /// `..._INGAME_NONPERSISTENT` (3).
    IngameNonpersistent,
    /// `..._RENTAL` (4).
    Rental,
    /// `..._ARENA_COMMANDER` (5).
    ArenaCommander,
    /// A value not in the known schema.
    Unknown(i32),
}

impl EntitlementSource {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Platform,
            2 => Self::IngamePersistent,
            3 => Self::IngameNonpersistent,
            4 => Self::Rental,
            5 => Self::ArenaCommander,
            other => Self::Unknown(other),
        }
    }
}

/// Delivery status of an entitlement. `Unknown` covers `UNSPECIFIED` and any
/// future value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntitlementStatus {
    /// `..._PACKAGED` (1).
    Packaged,
    /// `..._UNDELIVERED` (2).
    Undelivered,
    /// `..._DELIVERED` (3).
    Delivered,
    /// `..._REVOKED` (4).
    Revoked,
    /// `..._FAILED` (5).
    Failed,
    /// A value not in the known schema.
    Unknown(i32),
}

impl EntitlementStatus {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Packaged,
            2 => Self::Undelivered,
            3 => Self::Delivered,
            4 => Self::Revoked,
            5 => Self::Failed,
            other => Self::Unknown(other),
        }
    }
}

/// One entitlement (owned-item grant) as reported by the backend. Ids are
/// wire-native: `class_guid` is the full item-class GUID string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entitlement {
    /// The entitlement record's URN.
    pub urn: String,
    /// Where the grant came from.
    pub source: EntitlementSource,
    /// Delivery status.
    pub status: EntitlementStatus,
    /// The entitled item-class GUID (full GUID string, wire-native).
    pub class_guid: String,
    /// Instantiated entity geid once materialised into the world; else `None`.
    pub geid: Option<u64>,
    /// Stack size from the entitlement metadata.
    pub stack_size: i32,
    /// Whether the grant was funded with real currency.
    pub real_money: bool,
    /// Rental expiry as Unix seconds, or `None` if not a rental / unset.
    pub rental_expiry: Option<i64>,
}

impl Entitlement {
    fn from_proto(e: &ProtoEntitlement) -> Self {
        let (stack_size, real_money) = e
            .metadata
            .as_ref()
            .map(|m| (m.stack_size, m.real_money))
            .unwrap_or((0, false));

        Self {
            urn: e.urn.clone(),
            source: EntitlementSource::from_i32(e.source_type),
            status: EntitlementStatus::from_i32(e.status),
            class_guid: e.entity_class_guid.clone(),
            geid: e.entity_geid,
            stack_size,
            real_money,
            rental_expiry: e.rental_expiry.as_ref().map(|t| t.seconds),
        }
    }
}

/// Fetch every entitlement for the current player, concatenating the
/// server-streamed pages. `jwt` is the player-scoped token.
pub(crate) async fn query(channel: &Channel, jwt: &str) -> Result<Vec<Entitlement>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ExternalEntitlementServiceClient::with_interceptor(channel.clone(), auth);

    // Empty/unset filter → everything the player owns.
    let request = QueryEntitlementsStreamRequest { filter: None };
    let mut stream = client.query_entitlements_stream(request).await?.into_inner();

    let mut out = Vec::new();
    while let Some(response) = stream.message().await? {
        out.extend(response.results.iter().map(Entitlement::from_proto));
    }

    Ok(out)
}
