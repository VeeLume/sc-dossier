//! Stowed-item / inventory queries and the public [`Item`] / [`Inventory`]
//! types — every stowed entity, each tagged with its [`Location`]. Returns raw
//! IDs; CRC→name resolution is the consumer's (see [`Item::class_crc`], [`Context`]).

use std::collections::{HashMap, HashSet};

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::common_api::PaginationArguments;
use crate::proto::entitygraph::entity_graph_service_client::EntityGraphServiceClient;
use crate::proto::entitygraph::{
    entity_filter, inventory_configuration, node_properties, scalar_value, EdgeFilter,
    EntityFilter, EntityGraphQuery, EntityNodeProperties, EntityProjection, EntityQueryRequest,
    EntityQueryRequestBody, EntitySnapshot, EntityTreeProjection, GetInventoriesRequest,
    GetInventoriesRequestBody, InventoryNodeProperties, ScalarValue, Scope, ScopeType,
};

/// Inventory page size for the entity-graph cursor.
const PAGE_SIZE: u32 = 250;

/// `name_crc` of the snapshot variable carrying the resource descriptor (cargo
/// only). See [`decode_resource`].
const RESOURCE_DESCRIPTOR_CRC: u32 = 4_185_637_390;

/// Where an item or inventory lives. `Location`/`Hangar` carry a place-GUID CRC
/// (→ holotable `StarMapObject` by CRC); `Container` carries a ship geid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Context {
    /// The character's personal inventory (`"PlayerInventory"`).
    Player,
    /// Bound to an entitlement (`"Entitlement"`).
    Entitlement,
    /// A world location; the `u32` is a place-GUID CRC (`"Location"`).
    Location(u32),
    /// A hangar; the `u32` is a place-GUID CRC (`"Hangar"`).
    Hangar(u32),
    /// Inside a ship/container; the `u64` is the container's geid (`"Container"`).
    Container(u64),
    /// Any other / future context kind, carrying the raw `context` string.
    Other(String),
}

impl Context {
    /// Classify an inventory node's `context` string. `"Container"` takes the
    /// ship geid from `owner_id`, not `subject_id`.
    fn from_inventory(context: &str, subject_id: u64, owner_id: u64) -> Self {
        match context {
            "PlayerInventory" => Self::Player,
            "Entitlement" => Self::Entitlement,
            "Location" => Self::Location(subject_id as u32),
            "Hangar" => Self::Hangar(subject_id as u32),
            "Container" => Self::Container(owner_id),
            other => Self::Other(other.to_string()),
        }
    }
}

/// Where a stowed item sits: the owning inventory's id plus its [`Context`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Location {
    /// The inventory's unique id (the entity-graph sharding key).
    pub inventory_id: String,
    /// The kind of place the inventory represents.
    pub context: Context,
}

/// Material overlay for a cargo item. Only present on resource/material stacks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Resource {
    /// Resource-type id (→ holotable `ResourceType` by CRC).
    pub resource_id: u32,
    /// Quality, `0..=1000`.
    pub quality: u16,
    /// Quantity in SCU (standard cargo units).
    pub scu: f64,
}

/// One stowed item (a single entity instance), tagged with its [`Location`].
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    /// Instance id (global entity id).
    pub geid: u64,
    /// Item-class GUID CRC (→ holotable item name by CRC).
    pub class_crc: u32,
    /// `EItemType` index — cheap pre-holotable filter (Cargo=16, WeaponGun=187).
    pub item_type_enum: i32,
    /// Where this item is stowed.
    pub location: Location,
    /// Quantity for discrete/stackable items (always at least 1).
    pub stack_size: u32,
    /// The entitlement URN this instance is bound to, if any.
    pub parent_urn: Option<String>,
    /// Material overlay, for cargo/resource stacks only.
    pub resource: Option<Resource>,
}

/// The kind of an inventory container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InventoryType {
    /// `..._INFINITE` (1) — infinite "bag of holding".
    Infinite,
    /// `..._PHYSICAL` (2) — has a volumetric capacity.
    Physical,
    /// A value not in the known schema.
    Unknown(i32),
}

impl InventoryType {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::Infinite,
            2 => Self::Physical,
            other => Self::Unknown(other),
        }
    }
}

/// An inventory container (including empty ones), with type and capacity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inventory {
    /// Where this inventory lives.
    pub location: Location,
    /// The legal owner's id (e.g. the ship geid for a container).
    pub owner_id: u64,
    /// The inventory kind (infinite / physical).
    pub inventory_type: InventoryType,
    /// Volumetric capacity, for physical inventories only.
    pub capacity: Option<i32>,
    /// Current volumetric occupancy, for physical inventories only.
    pub occupancy: Option<i32>,
}

impl Inventory {
    fn from_proto(p: &InventoryNodeProperties) -> Self {
        let (capacity, occupancy) = match p.configuration.as_ref().and_then(|c| c.configuration.as_ref()) {
            Some(inventory_configuration::Configuration::Physical(phys)) => {
                (Some(phys.capacity), Some(phys.occupancy))
            }
            None => (None, None),
        };

        Self {
            location: Location {
                inventory_id: p.id.clone(),
                context: Context::from_inventory(&p.context, p.subject_id, p.owner_id),
            },
            owner_id: p.owner_id,
            inventory_type: InventoryType::from_i32(p.inventory_type),
            capacity,
            occupancy,
        }
    }
}

/// Every stowed item across all of the current player's inventories, each
/// tagged with its [`Location`]. `jwt` is the **player-scoped** token; `geid`
/// is the player's character geid.
pub(crate) async fn query(channel: &Channel, jwt: &str, geid: u64) -> Result<Vec<Item>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = EntityGraphServiceClient::with_interceptor(channel.clone(), auth);

    let inventories = get_inventories(&mut client, geid).await?;

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    for inv in &inventories {
        let location = Location {
            inventory_id: inv.id.clone(),
            context: Context::from_inventory(&inv.context, inv.subject_id, inv.owner_id),
        };
        collect_items(&mut client, &location, &mut out, &mut seen).await?;
    }

    Ok(out)
}

/// Inventory containers owned by the current player (including empty ones), with
/// type and capacity. Secondary to [`query`].
pub(crate) async fn inventories(channel: &Channel, jwt: &str, geid: u64) -> Result<Vec<Inventory>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = EntityGraphServiceClient::with_interceptor(channel.clone(), auth);

    let inventories = get_inventories(&mut client, geid).await?;
    Ok(inventories.iter().map(Inventory::from_proto).collect())
}

/// Fetch the player's inventory nodes via `GetInventories`, keeping only the
/// inventory-typed nodes.
async fn get_inventories(
    client: &mut EntityGraphServiceClient<
        tonic::service::interceptor::InterceptedService<Channel, BearerAuth>,
    >,
    geid: u64,
) -> Result<Vec<InventoryNodeProperties>> {
    let request = GetInventoriesRequest {
        body: Some(GetInventoriesRequestBody { owner_id: geid.to_string() }),
    };
    let body = client.get_inventories(request).await?.into_inner().body.unwrap_or_default();

    Ok(body
        .inventories
        .into_iter()
        .filter_map(|node| match node.properties?.properties? {
            node_properties::Properties::InventoryProperties(p) => Some(p),
            node_properties::Properties::EntityProperties(_) => None,
        })
        .collect())
}

/// Walk the entity-graph cursor for one inventory, appending each stowed item
/// (deduped by geid — the inclusive cursor re-emits boundary rows).
async fn collect_items(
    client: &mut EntityGraphServiceClient<
        tonic::service::interceptor::InterceptedService<Channel, BearerAuth>,
    >,
    location: &Location,
    out: &mut Vec<Item>,
    seen: &mut HashSet<u64>,
) -> Result<()> {
    let mut after = String::new();

    loop {
        let request = entity_query_request(&location.inventory_id, after.clone());
        let body = client.entity_query(request).await?.into_inner().body.unwrap_or_default();

        // Resource descriptors live in the per-page snapshots, keyed by entity id.
        let resources: HashMap<u64, Resource> = body
            .snapshots
            .iter()
            .filter_map(|s| Some((s.entity_id, decode_resource(s)?)))
            .collect();

        if let Some(graph) = body.results {
            for node in graph.nodes {
                let Some(node_properties::Properties::EntityProperties(e)) =
                    node.properties.and_then(|p| p.properties)
                else {
                    continue;
                };
                if seen.insert(e.geid) {
                    out.push(item_from_proto(&e, location, resources.get(&e.geid).copied()));
                }
            }
        }

        match body.page_info {
            Some(info) if info.has_next_page && !info.end_cursor.is_empty() => {
                after = info.end_cursor;
            }
            _ => break,
        }
    }

    Ok(())
}

/// Build the `EntityQuery` for one inventory: an `EdgeFilter` on the literal
/// `"STOWED_IN"` string, GLOBAL scope with the inventory id echoed into
/// `query.inventory_id`. Wrong shape → validation error or 20s global-walk.
fn entity_query_request(inventory_id: &str, after: String) -> EntityQueryRequest {
    EntityQueryRequest {
        body: Some(EntityQueryRequestBody {
            scope: Some(Scope { r#type: ScopeType::Global as i32, shard_id: String::new() }),
            query: Some(EntityGraphQuery {
                filter: Some(EntityFilter {
                    filter_type: Some(entity_filter::FilterType::EdgeFilter(EdgeFilter {
                        edge_type: "STOWED_IN".to_string(),
                        values: vec![ScalarValue {
                            scalar_type: Some(scalar_value::ScalarType::StringValue(
                                inventory_id.to_string(),
                            )),
                        }],
                    })),
                }),
                pagination: Some(PaginationArguments { first: PAGE_SIZE, after }),
                projection: Some(EntityProjection {
                    // Must be present or validation fails; disabled = no tree walk.
                    tree: Some(EntityTreeProjection::default()),
                    // Needed for the resource descriptor.
                    snapshots: true,
                    // Holotable resolves names by CRC; we don't need server classes.
                    entity_classes: false,
                    ..Default::default()
                }),
                // Same id, required for GLOBAL scope.
                inventory_id: inventory_id.to_string(),
                language: "en".to_string(),
                ..Default::default()
            }),
        }),
    }
}

fn item_from_proto(e: &EntityNodeProperties, location: &Location, resource: Option<Resource>) -> Item {
    Item {
        geid: e.geid,
        class_crc: e.class_guid_crc,
        item_type_enum: e.item_type_enum,
        location: location.clone(),
        stack_size: e.stack_size.max(1),
        parent_urn: (!e.parent_urn.is_empty()).then(|| e.parent_urn.clone()),
        resource,
    }
}

/// Decode the material/resource descriptor from a snapshot's variables. Layout:
/// `[0]=1` (version) | `[1..5]` u32 LE resource_id | `[5..7]` u16 LE quality |
/// `[7..11]` u32 LE microSCU. Absent or `version != 1` ⇒ `None`.
fn decode_resource(snapshot: &EntitySnapshot) -> Option<Resource> {
    let bytes = &snapshot
        .variables
        .iter()
        .find(|v| v.name_crc == RESOURCE_DESCRIPTOR_CRC)?
        .snapshot;

    if bytes.len() < 11 || bytes[0] != 1 {
        return None;
    }

    let resource_id = u32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
    let quality = u16::from_le_bytes([bytes[5], bytes[6]]);
    let micro_scu = u32::from_le_bytes([bytes[7], bytes[8], bytes[9], bytes[10]]);

    Some(Resource { resource_id, quality, scu: micro_scu as f64 / 1_000_000.0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::entitygraph::EntityVariable;

    fn descriptor_var(bytes: Vec<u8>) -> EntitySnapshot {
        EntitySnapshot {
            entity_id: 1,
            variables: vec![EntityVariable {
                name_crc: RESOURCE_DESCRIPTOR_CRC,
                r#type: 0,
                flags: 0,
                snapshot: bytes,
            }],
            version: 0,
        }
    }

    #[test]
    fn decode_resource_reads_le_fields() {
        // version=1, resource_id=0x04030201, quality=1000 (0x03E8), microSCU=2_000_000.
        let mut bytes = vec![1u8];
        bytes.extend_from_slice(&0x04030201u32.to_le_bytes());
        bytes.extend_from_slice(&1000u16.to_le_bytes());
        bytes.extend_from_slice(&2_000_000u32.to_le_bytes());

        let r = decode_resource(&descriptor_var(bytes)).expect("decodes");
        assert_eq!(r.resource_id, 0x04030201);
        assert_eq!(r.quality, 1000);
        assert_eq!(r.scu, 2.0);
    }

    #[test]
    fn decode_resource_rejects_bad_version_and_short() {
        let mut wrong_version = vec![2u8];
        wrong_version.extend_from_slice(&[0u8; 10]);
        assert!(decode_resource(&descriptor_var(wrong_version)).is_none());

        assert!(decode_resource(&descriptor_var(vec![1, 0, 0])).is_none());
    }

    #[test]
    fn decode_resource_absent_variable() {
        let snapshot = EntitySnapshot { entity_id: 1, variables: vec![], version: 0 };
        assert!(decode_resource(&snapshot).is_none());
    }

    #[test]
    fn context_classifies_known_kinds() {
        assert_eq!(Context::from_inventory("PlayerInventory", 0, 0), Context::Player);
        assert_eq!(Context::from_inventory("Entitlement", 0, 0), Context::Entitlement);
        assert_eq!(Context::from_inventory("Location", 42, 0), Context::Location(42));
        assert_eq!(Context::from_inventory("Hangar", 7, 0), Context::Hangar(7));
        // Container uses owner_id (ship geid), not subject_id.
        assert_eq!(Context::from_inventory("Container", 1, 99), Context::Container(99));
        assert_eq!(
            Context::from_inventory("Weird", 0, 0),
            Context::Other("Weird".to_string())
        );
    }
}
