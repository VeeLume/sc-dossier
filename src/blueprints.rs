//! Owned-blueprint query and the public [`Blueprint`] type. Returns raw IDs;
//! GUID→name resolution is the consumer's responsibility.

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::blueprint_library::blueprint_library_service_client::BlueprintLibraryServiceClient;
use crate::proto::blueprint_library::{BlueprintEntry as ProtoEntry, QueryBlueprintEntriesRequest};
use crate::proto::common_api::{PaginationArguments, Query};

/// Never 100: the backend silently drops entries and mis-reports
/// `has_next_page` at exactly 100. Always walk the cursor.
const PAGE_SIZE: u32 = 50;

/// Where a blueprint came from. `Unknown` carries any future/unrecognised value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintSource {
    /// `BLUEPRINT_SOURCE_UNSPECIFIED` (0).
    Unspecified,
    /// `BLUEPRINT_SOURCE_GAMEPLAY` (1).
    Gameplay,
    /// `BLUEPRINT_SOURCE_PLATFORM` (2).
    Platform,
    /// A value not in the known schema.
    Unknown(i32),
}

impl BlueprintSource {
    fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::Gameplay,
            2 => Self::Platform,
            other => Self::Unknown(other),
        }
    }
}

/// The crafting process a blueprint drives. `Unknown` carries any future value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintProcessType {
    /// `..._UNSPECIFIED` (0).
    Unspecified,
    /// `..._CREATE` (1).
    Create,
    /// `..._REFINE` (2).
    Refine,
    /// `..._REPAIR` (3).
    Repair,
    /// `..._UPGRADE` (4).
    Upgrade,
    /// `..._DISMANTLE` (5).
    Dismantle,
    /// `..._RESEARCH` (6).
    Research,
    /// A value not in the known schema.
    Unknown(i32),
}

impl BlueprintProcessType {
    fn from_i32(v: i32) -> Self {
        match v {
            0 => Self::Unspecified,
            1 => Self::Create,
            2 => Self::Refine,
            3 => Self::Repair,
            4 => Self::Upgrade,
            5 => Self::Dismantle,
            6 => Self::Research,
            other => Self::Unknown(other),
        }
    }
}

/// One owned blueprint as reported by the backend. IDs are GUIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blueprint {
    /// The blueprint record id.
    pub blueprint_id: String,
    /// The blueprint's category id.
    pub category_id: String,
    /// The crafted item class id.
    pub item_class_id: String,
    /// Blueprint tier.
    pub tier: u32,
    /// Remaining uses; **`-1` means unlimited**.
    pub remaining_uses: i32,
    /// Where the blueprint came from.
    pub source: BlueprintSource,
    /// The crafting process this blueprint drives.
    pub process_type: BlueprintProcessType,
    /// Last-used time as Unix seconds, or `None` if never used / unset.
    pub last_used_at: Option<i64>,
}

impl Blueprint {
    fn from_proto(e: &ProtoEntry) -> Self {
        Self {
            blueprint_id: e.blueprint_id.clone(),
            category_id: e.category_id.clone(),
            item_class_id: e.item_class_id.clone(),
            tier: e.tier,
            remaining_uses: e.remaining_uses,
            source: BlueprintSource::from_i32(e.source),
            process_type: BlueprintProcessType::from_i32(e.process_type),
            last_used_at: e.last_used_at.as_ref().map(|t| t.seconds).filter(|s| *s != 0),
        }
    }
}

/// Fetch every owned blueprint for the authenticated account, walking the
/// cursor to completion. `jwt` is the account-scoped game token; `channel` is a
/// connected gRPC channel to the services endpoint.
pub(crate) async fn owned(channel: &Channel, jwt: &str) -> Result<Vec<Blueprint>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = BlueprintLibraryServiceClient::with_interceptor(channel.clone(), auth);

    let mut after = String::new();
    let mut out = Vec::new();

    loop {
        let request = QueryBlueprintEntriesRequest {
            query: Some(Query {
                filter: None,
                pagination: Some(PaginationArguments { first: PAGE_SIZE, after: after.clone() }),
                sort: None,
            }),
        };

        let response = client.query_blueprint_entries(request).await?.into_inner();
        out.extend(response.results.iter().map(Blueprint::from_proto));

        match response.page_info {
            Some(info) if info.has_next_page && !info.end_cursor.is_empty() => {
                after = info.end_cursor;
            }
            _ => break,
        }
    }

    Ok(out)
}
