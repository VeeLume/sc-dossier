//! Saved loadouts from entitlement snapshots: per owned item (ship, weapon,
//! armour), what sits on each port. `Saved` is the loadout a claim restores.

use std::collections::BTreeMap;

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::entitlement::external_entitlement_service_client::ExternalEntitlementServiceClient;
use crate::proto::entitlement::{QuerySnapshotsStreamRequest, SnapshotFilter, SnapshotSlotId};

/// The loadout of one owned root item in one slot.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Loadout {
    /// Entitlement URN of the root item.
    pub root_urn: String,
    /// Which loadout.
    pub slot: LoadoutSlot,
    /// Attached items, sorted by port path.
    pub entries: Vec<LoadoutEntry>,
}

/// Loadout slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[non_exhaustive]
pub enum LoadoutSlot {
    /// Stock loadout.
    Stock,
    /// The player's saved loadout (claim target); only present once changed.
    Saved,
    /// A value not in the known schema.
    Other(i32),
}

/// One attached item.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct LoadoutEntry {
    /// Item-port names from the root down.
    pub path: Vec<String>,
    /// What occupies the port.
    pub item: LoadoutItem,
}

/// Occupant of a loadout port.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LoadoutItem {
    /// An owned entitlement (URN).
    Entitlement(String),
    /// An entity class without its own entitlement (GUID), e.g. missiles.
    Class(String),
    /// The port is empty.
    Empty,
    /// An unrecognised URN.
    Other(String),
}

impl LoadoutItem {
    fn from_urn(urn: &str) -> Self {
        if urn.is_empty() {
            Self::Empty
        } else if let Some(guid) = urn.strip_prefix("urn:sc:global:entityclass:uuid:") {
            Self::Class(guid.to_string())
        } else if urn.contains(":entitlement:") {
            Self::Entitlement(urn.to_string())
        } else {
            Self::Other(urn.to_string())
        }
    }
}

impl LoadoutSlot {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Stock,
            2 => Self::Saved,
            other => Self::Other(other),
        }
    }
}

pub(crate) async fn query(channel: &Channel, jwt: &str) -> Result<Vec<Loadout>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ExternalEntitlementServiceClient::with_interceptor(channel.clone(), auth);
    let filter = SnapshotFilter {
        entitlement_urn: Vec::new(),
        snapshot_slot_id: vec![
            SnapshotSlotId::Default as i32,
            SnapshotSlotId::Customized as i32,
        ],
    };
    let mut stream = client
        .query_snapshots_stream(QuerySnapshotsStreamRequest {
            filter: Some(filter),
        })
        .await?
        .into_inner();

    let mut by_root: BTreeMap<(String, LoadoutSlot), Vec<LoadoutEntry>> = BTreeMap::new();
    while let Some(page) = stream.message().await? {
        for r in page.results {
            let entries = by_root
                .entry((r.root_urn, LoadoutSlot::from_i32(r.slot_id)))
                .or_default();
            // The root's own record (empty path) is the root itself.
            if !r.attach_path.is_empty() {
                entries.push(LoadoutEntry {
                    path: r.attach_path,
                    item: LoadoutItem::from_urn(&r.item_urn),
                });
            }
        }
    }

    Ok(by_root
        .into_iter()
        .map(|((root_urn, slot), mut entries)| {
            entries.sort_by(|a, b| a.path.cmp(&b.path));
            Loadout {
                root_urn,
                slot,
                entries,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_urns() {
        assert_eq!(LoadoutItem::from_urn(""), LoadoutItem::Empty);
        assert_eq!(
            LoadoutItem::from_urn(
                "urn:sc:global:entityclass:uuid:b86bd2f2-1f5e-4731-a286-74faa8cde646"
            ),
            LoadoutItem::Class("b86bd2f2-1f5e-4731-a286-74faa8cde646".into())
        );
        for u in [
            "urn:sc:global:entitlement:uuid:x",
            "urn:sc:platform:entitlement:uuid:x",
        ] {
            assert_eq!(LoadoutItem::from_urn(u), LoadoutItem::Entitlement(u.into()));
        }
    }
}
