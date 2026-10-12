//! Item trees per inventory: what is attached to and stowed inside each item.
//! Returns raw IDs; CRC→name resolution is the consumer's.

use std::collections::{HashMap, HashSet};

use tonic::service::interceptor::InterceptedService;
use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::{Error, Result};
use crate::items::{decode_resource, get_inventories, Context, Location, Resource};
use crate::proto::common_api::PaginationArguments;
use crate::proto::entitygraph::entity_edge_vertex::Id as VertexId;
use crate::proto::entitygraph::entity_graph_service_client::EntityGraphServiceClient;
use crate::proto::entitygraph::{
    entity_filter, node_properties, scalar_value, EdgeFilter, EntityEdge, EntityEdgeType,
    EntityFilter, EntityGraphQuery, EntityNodeProperties, EntityProjection, EntityPruneConstraint,
    EntityQueryRequest, EntityQueryRequestBody, EntityTreeProjection, EntityVariable,
    InventoryNodeProperties, PropertyFilter, ScalarValue, Scope, ScopeType,
};

type Client = EntityGraphServiceClient<InterceptedService<Channel, BearerAuth>>;

/// Tree-query page size (per-tree cost dominates; keep pages small).
const PAGE_SIZE: u32 = 100;
/// Nested container levels to follow (bag in bag in bag).
const CONTENTS_DEPTH: usize = 3;
/// Guard against malformed (cyclic/deep) graphs.
const MAX_DEPTH: usize = 32;
/// `class_guid_crc` of `VehicleObjectContainer`: ship interiors, pruned.
const INTERIOR_CRC: u32 = 1_595_545_140;
/// Snapshot var `ac.ammocount` (CRC32 of the lowercased name).
const AMMO_VAR: u32 = 0x004e_260a;
/// Snapshot var `crafting.persistentitemdata`.
const CRAFTED_VAR: u32 = 0x8198_c4b7;

/// All of the player's inventories as item trees.
#[derive(Debug)]
#[non_exhaustive]
pub struct Stashes {
    /// One entry per inventory that loaded.
    pub stashes: Vec<Stash>,
    /// Inventories whose query failed, with the error.
    pub failed: Vec<(Location, Error)>,
}

/// One inventory and the item trees stowed in it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Stash {
    /// The inventory.
    pub location: Location,
    /// Items stowed directly in it.
    pub items: Vec<ItemNode>,
}

/// One item instance with everything attached to and stowed inside it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct ItemNode {
    /// Instance id.
    pub geid: u64,
    /// Item-class GUID CRC (→ holotable by CRC).
    pub class_crc: u32,
    /// `EItemType` index.
    pub item_type_enum: i32,
    /// Quantity (at least 1).
    pub stack_size: u32,
    /// Bound entitlement URN, if any.
    pub parent_urn: Option<String>,
    /// Port on the parent item (see [`port_crc`]); `None` when stowed.
    pub port_crc: Option<u32>,
    /// Ownership state.
    pub ownership: Ownership,
    /// Material overlay, for cargo/resource stacks.
    pub resource: Option<Resource>,
    /// Rounds left, for magazines.
    pub ammo: Option<u32>,
    /// Crafting record, for crafted items.
    pub crafted: Option<Crafted>,
    /// Items attached to this one's ports.
    pub attached: Vec<ItemNode>,
    /// Items stowed in this one's own inventory.
    pub contents: Vec<ItemNode>,
}

/// Ownership state; `since` is unix seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Ownership {
    /// No ownership state.
    None,
    /// Bound to the player.
    Imprinted {
        /// When.
        since: Option<i64>,
    },
    /// Rented.
    Rented {
        /// When.
        since: Option<i64>,
    },
    /// Unusable, e.g. the old instance after a claim elsewhere.
    Bricked {
        /// When.
        since: Option<i64>,
    },
    /// A value not in the known schema.
    Other(i32),
}

/// Crafting record of a crafted item.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct Crafted {
    /// Inputs.
    pub ingredients: Vec<Ingredient>,
    /// Crafted stat values (`CraftingGameplayPropertyDef` CRC, value).
    pub properties: Vec<(u32, f32)>,
    /// Output entity-class CRC.
    pub output_class_crc: u32,
}

/// One crafting input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ingredient {
    /// Resource or item.
    pub kind: IngredientKind,
    /// `ResourceType` or entity-class CRC.
    pub class_crc: u32,
    /// Quality, `0..=1000`.
    pub quality: u16,
    /// µSCU for resources, count for items.
    pub amount: u64,
}

/// Kind of crafting input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum IngredientKind {
    /// A resource; amount in µSCU.
    Resource,
    /// An item; amount is a count.
    Item,
    /// A value not in the known schema.
    Other(u8),
}

/// Wire CRC of an item-port name (`attachment_port_crc`).
pub fn port_crc(name: &str) -> u32 {
    crc32c(name.to_ascii_lowercase().as_bytes())
}

pub(crate) async fn all(channel: &Channel, jwt: &str, geid: u64) -> Result<Stashes> {
    let mut client =
        EntityGraphServiceClient::with_interceptor(channel.clone(), BearerAuth::new(jwt)?);
    let mut out = Stashes {
        stashes: Vec::new(),
        failed: Vec::new(),
    };
    for inv in get_inventories(&mut client, geid).await? {
        let location = Location {
            inventory_id: inv.id.clone(),
            context: Context::from_inventory(&inv.context, inv.subject_id, inv.owner_id),
        };
        match load(&mut client, &location).await {
            Ok(stash) => out.stashes.push(stash),
            Err(e) => out.failed.push((location, e)),
        }
    }
    Ok(out)
}

pub(crate) async fn one(channel: &Channel, jwt: &str, location: &Location) -> Result<Stash> {
    let mut client =
        EntityGraphServiceClient::with_interceptor(channel.clone(), BearerAuth::new(jwt)?);
    load(&mut client, location).await
}

async fn load(client: &mut Client, location: &Location) -> Result<Stash> {
    let mut g = Graph::default();
    g.query(client, &location.inventory_id).await?;

    // Nested containers: each inventory's contents need their own query.
    let mut done = HashSet::new();
    for _ in 0..CONTENTS_DEPTH {
        let todo: Vec<String> = g
            .inventories
            .values()
            .filter(|i| i.context == "Container" && !done.contains(&i.id))
            .map(|i| i.id.clone())
            .collect();
        if todo.is_empty() {
            break;
        }
        for id in todo {
            done.insert(id.clone());
            g.query(client, &id).await?;
        }
    }

    Ok(Stash {
        location: location.clone(),
        items: g.stowed_in(&location.inventory_id, 0, &mut HashSet::new()),
    })
}

#[derive(Default)]
struct Graph {
    entities: HashMap<u64, EntityNodeProperties>,
    inventories: HashMap<String, InventoryNodeProperties>,
    snapshots: HashMap<u64, Vec<EntityVariable>>,
    /// parent geid -> (child geid, port crc)
    attached: HashMap<u64, Vec<(u64, u32)>>,
    /// inventory id -> stowed geids
    stowed: HashMap<String, Vec<u64>>,
    /// entity geid -> its own inventory ids
    owns: HashMap<u64, Vec<String>>,
    seen_edges: HashSet<(i32, String, String)>,
}

impl Graph {
    async fn query(&mut self, client: &mut Client, inventory_id: &str) -> Result<()> {
        let mut after = String::new();
        loop {
            let body = client
                .entity_query(tree_request(inventory_id, after.clone()))
                .await?
                .into_inner()
                .body
                .unwrap_or_default();
            for s in body.snapshots {
                self.snapshots.insert(s.entity_id, s.variables);
            }
            if let Some(r) = body.results {
                for n in r.nodes {
                    match n.properties.and_then(|p| p.properties) {
                        Some(node_properties::Properties::EntityProperties(e)) => {
                            self.entities.insert(e.geid, e);
                        }
                        Some(node_properties::Properties::InventoryProperties(i)) => {
                            self.inventories.entry(i.id.clone()).or_insert(i);
                        }
                        None => {}
                    }
                }
                for e in r.edges {
                    self.edge(e);
                }
            }
            match body.page_info {
                Some(p) if p.has_next_page && !p.end_cursor.is_empty() => after = p.end_cursor,
                _ => return Ok(()),
            }
        }
    }

    /// Index one edge (`start` = child, `end` = parent), deduped across pages.
    fn edge(&mut self, e: EntityEdge) {
        let (Some(start), Some(end)) = (vertex(&e.start), vertex(&e.end)) else {
            return;
        };
        let key = (e.r#type, format!("{start:?}"), format!("{end:?}"));
        if !self.seen_edges.insert(key) {
            return;
        }
        let port = e.properties.map_or(0, |p| p.attachment_port_crc);
        match (EntityEdgeType::try_from(e.r#type), start, end) {
            (Ok(EntityEdgeType::AttachedTo), V::Entity(c), V::Entity(p)) => {
                self.attached.entry(p).or_default().push((c, port))
            }
            (Ok(EntityEdgeType::StowedIn), V::Entity(c), V::Inventory(i)) => {
                self.stowed.entry(i).or_default().push(c)
            }
            (Ok(EntityEdgeType::References), V::Inventory(i), V::Entity(p)) => {
                self.owns.entry(p).or_default().push(i)
            }
            _ => {}
        }
    }

    fn stowed_in(
        &self,
        inventory_id: &str,
        depth: usize,
        path: &mut HashSet<u64>,
    ) -> Vec<ItemNode> {
        self.stowed
            .get(inventory_id)
            .into_iter()
            .flatten()
            .filter_map(|g| self.node(*g, None, depth, path))
            .collect()
    }

    fn node(
        &self,
        geid: u64,
        port: Option<u32>,
        depth: usize,
        path: &mut HashSet<u64>,
    ) -> Option<ItemNode> {
        let e = self.entities.get(&geid)?;
        if depth > MAX_DEPTH || !path.insert(geid) {
            return None;
        }
        let attached = self
            .attached
            .get(&geid)
            .into_iter()
            .flatten()
            .filter_map(|(c, p)| self.node(*c, Some(*p), depth + 1, path))
            .collect();
        let contents = self
            .owns
            .get(&geid)
            .into_iter()
            .flatten()
            .filter(|i| {
                self.inventories
                    .get(*i)
                    .is_some_and(|n| n.context == "Container")
            })
            .flat_map(|i| self.stowed_in(i, depth + 1, path))
            .collect();
        path.remove(&geid);

        let vars = self.snapshots.get(&geid);
        let var = |crc: u32| {
            vars.and_then(|v| v.iter().find(|x| x.name_crc == crc))
                .map(|x| x.snapshot.as_slice())
        };
        Some(ItemNode {
            geid,
            class_crc: e.class_guid_crc,
            item_type_enum: e.item_type_enum,
            stack_size: e.stack_size.max(1),
            parent_urn: (!e.parent_urn.is_empty()).then(|| e.parent_urn.clone()),
            port_crc: port,
            ownership: ownership(e),
            resource: vars.and_then(|v| {
                decode_resource(&crate::proto::entitygraph::EntitySnapshot {
                    entity_id: geid,
                    variables: v.clone(),
                    version: 0,
                })
            }),
            ammo: var(AMMO_VAR).and_then(|b| Some(u32::from_le_bytes(b.try_into().ok()?))),
            crafted: var(CRAFTED_VAR).and_then(parse_crafted),
            attached,
            contents,
        })
    }
}

#[derive(Debug)]
enum V {
    Entity(u64),
    Inventory(String),
}

fn vertex(v: &Option<crate::proto::entitygraph::EntityEdgeVertex>) -> Option<V> {
    match v.as_ref()?.id.as_ref()? {
        VertexId::EntityId(id) => Some(V::Entity(*id)),
        VertexId::InventoryId(id) => Some(V::Inventory(id.clone())),
        VertexId::ShardId(_) => None,
    }
}

fn ownership(e: &EntityNodeProperties) -> Ownership {
    let since = e.ownership_timestamp.as_ref().map(|t| t.seconds);
    match e.ownership_status {
        0 | 1 => Ownership::None,
        2 => Ownership::Imprinted { since },
        3 => Ownership::Rented { since },
        4 => Ownership::Bricked { since },
        other => Ownership::Other(other),
    }
}

/// `STOWED_IN` scoping filter + attachment tree, pruned at ship interiors
/// (required for big ships; also lets inventory nodes be included).
fn tree_request(inventory_id: &str, after: String) -> EntityQueryRequest {
    let string = |s: &str| ScalarValue {
        scalar_type: Some(scalar_value::ScalarType::StringValue(s.to_string())),
    };
    EntityQueryRequest {
        body: Some(EntityQueryRequestBody {
            scope: Some(Scope {
                r#type: ScopeType::Global as i32,
                shard_id: String::new(),
            }),
            query: Some(EntityGraphQuery {
                filter: Some(EntityFilter {
                    filter_type: Some(entity_filter::FilterType::EdgeFilter(EdgeFilter {
                        edge_type: "STOWED_IN".to_string(),
                        values: vec![string(inventory_id)],
                    })),
                }),
                pagination: Some(PaginationArguments {
                    first: PAGE_SIZE,
                    after,
                }),
                projection: Some(EntityProjection {
                    tree: Some(EntityTreeProjection {
                        enabled: true,
                        prune: Some(EntityPruneConstraint {
                            from_root_node: true,
                            from: Some(EntityFilter {
                                filter_type: Some(entity_filter::FilterType::PropertyFilter(
                                    PropertyFilter {
                                        property: "class_guid_crc".to_string(),
                                        operator:
                                            crate::proto::entitygraph::ComparisonOperator::Equal
                                                as i32,
                                        values: vec![ScalarValue {
                                            scalar_type: Some(
                                                scalar_value::ScalarType::UnsignedIntegerValue(
                                                    INTERIOR_CRC,
                                                ),
                                            ),
                                        }],
                                    },
                                )),
                            }),
                            ..Default::default()
                        }),
                        include_inventory_nodes: true,
                        ..Default::default()
                    }),
                    snapshots: true,
                    outgoing_edges: true,
                    ..Default::default()
                }),
                inventory_id: inventory_id.to_string(),
                language: "en".to_string(),
                ..Default::default()
            }),
        }),
    }
}

/// Decode `crafting.persistentitemdata`: `[u8 n]` n × `[u8 kind][guid][u16 q]
/// [u64 amount]`, `[u8 _]`, `[u8 m]` m × `[guid][f32]`, `[guid output]`. GUIDs
/// map to class CRCs (`crc32c` of the storage bytes). Any misfit ⇒ `None`.
fn parse_crafted(b: &[u8]) -> Option<Crafted> {
    let mut r = Reader { b, at: 0 };
    let n = r.u8()?;
    let mut ingredients = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let kind = match r.u8()? {
            0 => IngredientKind::Resource,
            1 => IngredientKind::Item,
            k => IngredientKind::Other(k),
        };
        ingredients.push(Ingredient {
            kind,
            class_crc: crc32c(r.take(16)?),
            quality: u16::from_le_bytes(r.take(2)?.try_into().ok()?),
            amount: u64::from_le_bytes(r.take(8)?.try_into().ok()?),
        });
    }
    r.u8()?;
    let m = r.u8()?;
    let mut properties = Vec::with_capacity(m as usize);
    for _ in 0..m {
        let crc = crc32c(r.take(16)?);
        properties.push((crc, f32::from_le_bytes(r.take(4)?.try_into().ok()?)));
    }
    let output_class_crc = crc32c(r.take(16)?);
    (r.at == b.len()).then_some(Crafted {
        ingredients,
        properties,
        output_class_crc,
    })
}

struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.at..self.at.checked_add(n)?)?;
        self.at += n;
        Some(s)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
}

/// CRC32C (Castagnoli), the entity-graph hash.
fn crc32c(bytes: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in bytes {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = (c >> 1) ^ (0x82F6_3B78 & 0u32.wrapping_sub(c & 1));
        }
    }
    !c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn port_crc_matches_wire() {
        assert_eq!(port_crc("magazine_attach"), 0x5276_3c6e);
        assert_eq!(port_crc("hardpoint_Jump_Drive"), 0xaf8e_b1f9);
    }

    #[test]
    fn crafted_p6lr() {
        let b = hex("0400ce4dfe49a0feca066dc6163358dcf0b0550360ea0000000000000091447aed78951861b0b8822fae3774978003204e000000000000000a409aac3ca386f370d2c8c71ffeb8a76a033075000000000000018d48ad9523d75d12a15c4454c9620f836303010000000000000000042e4964e54ca37e03e49558ac5a3e75b56a4d333f28421eb06c715611b8b95bbe307436816a4d333ff2468a48ce29c1cffbfd0ccd7292f79238f8923f224c8119d6878ff634cb222935ced6be6a4d333fd84c52978ab8f20161cab2b451d1739f");
        let c = parse_crafted(&b).expect("decodes");
        assert_eq!(c.ingredients.len(), 4);
        assert_eq!(c.ingredients[0].class_crc, 122_448_454); // Taranite
        assert_eq!(c.ingredients[0].quality, 853);
        assert_eq!(c.ingredients[0].amount, 60_000);
        assert_eq!(c.ingredients[3].kind, IngredientKind::Item);
        assert_eq!(c.properties.len(), 4);
        assert_eq!(c.properties[2].0, 4_080_288_208); // GPP_Weapon_Damage
        assert!((c.properties[2].1 - 1.1482).abs() < 1e-4);
        assert_eq!(c.output_class_crc, 2_570_876_231); // behr_sniper_ballistic_01
    }

    #[test]
    fn crafted_rejects_misfits() {
        let mut empty = vec![0u8, 0, 0];
        empty.extend([0u8; 16]);
        assert!(parse_crafted(&empty).is_some(), "no ingredients, no props");
        assert!(parse_crafted(&empty[..10]).is_none(), "truncated");
        empty.push(0);
        assert!(parse_crafted(&empty).is_none(), "trailing byte");
        assert!(parse_crafted(&[0xff]).is_none(), "count overruns");
    }

    #[test]
    fn ownership_states() {
        let mut e = EntityNodeProperties {
            ownership_status: 4,
            ..Default::default()
        };
        assert_eq!(ownership(&e), Ownership::Bricked { since: None });
        e.ownership_status = 1;
        assert_eq!(ownership(&e), Ownership::None);
    }
}
