//! Accepted missions (contracts): [`Mission`], via
//! [`crate::Dossier::active_missions`]. Location values are mission DataSet
//! CRCs, not record class CRCs (sc-holotable:
//! `sc_locations::Universe::by_mission_crc`).

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::mission::{
    mission_variable_value, ActiveMissionInfo as ProtoMission, MissionObjective as ProtoObjective,
    MissionVariableValue,
};
use crate::proto::mission_service::mission_service_client::MissionServiceClient;
use crate::proto::mission_service::{
    query_active_missions_request, query_missions_request, QueryActiveMissionsRequest,
    QueryMissionsRequest,
};

/// What a mission objective is, from its id prefix (`dropoff_…` / `pickup_…` /
/// `phase_…`). The contract structure the planner keys on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveKind {
    /// `pickup_<uuid>_<k>` — collect cargo.
    Pickup,
    /// `dropoff_<uuid>_<k>` — deliver cargo.
    Dropoff,
    /// `phase_<uuid>` — a phase marker, not a physical stop.
    Phase,
    /// Anything else (recovery nav points, etc.).
    Other,
}

impl ObjectiveKind {
    fn from_id(id: &str) -> Self {
        if id.starts_with("pickup_") {
            Self::Pickup
        } else if id.starts_with("dropoff_") {
            Self::Dropoff
        } else if id.starts_with("phase_") {
            Self::Phase
        } else {
            Self::Other
        }
    }
}

/// Server-authoritative objective state. `Unknown` covers `UNSPECIFIED` and any
/// future value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ObjectiveState {
    /// `..._INPROGRESS` (1).
    InProgress,
    /// `..._COMPLETED` (2).
    Completed,
    /// `..._FAILED` (3).
    Failed,
    /// `..._EXPIRED` (4).
    Expired,
    /// A value not in the known schema.
    Unknown(i32),
}

impl ObjectiveState {
    fn from_i32(v: i32) -> Self {
        match v {
            1 => Self::InProgress,
            2 => Self::Completed,
            3 => Self::Failed,
            4 => Self::Expired,
            other => Self::Unknown(other),
        }
    }
}

/// One sub-objective of an accepted mission. The `(mission_id, objective_id)`
/// pair is the join key shared with the game log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissionObjective {
    /// `dropoff_<uuid>_<k>` / `pickup_<uuid>_<k>` / `phase_<uuid>`.
    pub objective_id: String,
    /// Derived from the id prefix.
    pub kind: ObjectiveKind,
    /// Server-authoritative state.
    pub state: ObjectiveState,
    /// Boxes/SCU delivered so far (the real progress the log's `0/M` lacks).
    pub progress_current: i64,
    /// Manifest total for this leg.
    pub progress_max: i64,
}

impl MissionObjective {
    fn from_proto(o: &ProtoObjective) -> Self {
        Self {
            objective_id: o.objective_id.clone(),
            kind: ObjectiveKind::from_id(&o.objective_id),
            state: ObjectiveState::from_i32(o.state),
            progress_current: o.progress_counter_current,
            progress_max: o.progress_counter_max,
        }
    }
}

/// One accepted mission (contract). Location values are **CRCs** (a closed
/// namespace — see the module docs); resolve them to names at the call site.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mission {
    /// The mission instance id (join key with the log, with `objective_id`).
    pub mission_id: String,
    /// The accepted-contract id.
    pub contract_id: String,
    /// The contract-template/definition id.
    pub contract_def_id: String,
    /// Pickup location CRCs in slot order (`PickupLocation1`, `2`, …).
    pub pickups: Vec<u32>,
    /// Dropoff location CRCs in slot order (`DropoffLocation1`, `2`, … — a
    /// single-dropoff contract yields one entry). Dropoff leg `k` targets
    /// `dropoffs[k % dropoffs.len()]`.
    pub dropoffs: Vec<u32>,
    /// The mission's sub-objectives.
    pub objectives: Vec<MissionObjective>,
}

impl Mission {
    fn from_proto(m: &ProtoMission) -> Self {
        // (slot index, crc)
        let mut pickup_slots: Vec<(u32, u32)> = Vec::new();
        let mut dropoff_slots: Vec<(u32, u32)> = Vec::new();
        let mut contract_def_id = String::new();

        if let Some(t) = &m.mission_template {
            contract_def_id = t.contract_definition_id.clone();
            if let Some(vars) = &t.variables {
                for e in &vars.variables {
                    let Some(crc) = e.value.as_ref().and_then(var_u32) else {
                        continue;
                    };
                    if let Some(slot) = location_slot(&e.key, "pickuplocation") {
                        pickup_slots.push((slot, crc));
                    } else if let Some(slot) = location_slot(&e.key, "dropofflocation") {
                        dropoff_slots.push((slot, crc));
                    }
                }
            }
        }

        Self {
            mission_id: m.mission_id.clone(),
            contract_id: m.contract_id.clone(),
            contract_def_id,
            pickups: by_slot(pickup_slots),
            dropoffs: by_slot(dropoff_slots),
            objectives: m
                .mission_objectives
                .iter()
                .map(MissionObjective::from_proto)
                .collect(),
        }
    }
}

/// Pull the numeric value out of a mission-template variable as a `u32`. Location
/// refs arrive as `integer_val` CRCs; `geid_val` is tolerated for safety.
fn var_u32(v: &MissionVariableValue) -> Option<u32> {
    match v.value.as_ref()? {
        mission_variable_value::Value::IntegerVal(i) => Some(*i as u32),
        mission_variable_value::Value::GeidVal(g) => Some(*g as u32),
        _ => None,
    }
}

/// The slot index of a `<prefix>[N][_BP]` key segment, or `None` if absent.
/// `prefix` is lowercase; matching ignores case. All of these occur live:
/// `DropoffLocation_BP` / `DropOffLocation_BP` / `DropoffLocation` → 1,
/// `DropoffLocation2_BP` → 2.
fn location_slot(key: &str, prefix: &str) -> Option<u32> {
    let seg = key.split('|').find(|s| {
        s.get(..prefix.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(prefix))
    })?;
    let rest = &seg[prefix.len()..];
    let mid = rest.strip_suffix("_BP").unwrap_or(rest);
    if mid.is_empty() {
        Some(1)
    } else {
        mid.parse().ok()
    }
}

fn by_slot(mut slots: Vec<(u32, u32)>) -> Vec<u32> {
    slots.sort_by_key(|(slot, _)| *slot);
    slots.into_iter().map(|(_, crc)| crc).collect()
}

/// Fetch the player's accepted missions. `jwt` is the player-scoped token,
/// `geid` the active character. Returns an empty vec when nothing is accepted.
pub(crate) async fn query(channel: &Channel, jwt: &str, geid: u64) -> Result<Vec<Mission>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = MissionServiceClient::with_interceptor(channel.clone(), auth);

    let urn = format!("urn:sc:global:player:geid:{geid}");
    let active = client
        .query_active_missions(QueryActiveMissionsRequest {
            queries: vec![query_active_missions_request::Body { urn }],
        })
        .await?
        .into_inner();

    let ids: Vec<String> = active
        .mission_ids
        .into_iter()
        .map(|b| b.mission_id)
        .collect();
    if ids.is_empty() {
        return Ok(Vec::new());
    }

    let full = client
        .query_missions(QueryMissionsRequest {
            queries: ids
                .into_iter()
                .map(|mission_id| query_missions_request::Body { mission_id })
                .collect(),
        })
        .await?
        .into_inner();

    Ok(full.missions.iter().map(Mission::from_proto).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn location_slot_indices() {
        let d = "dropofflocation";
        let p = "pickuplocation";
        assert_eq!(
            location_slot("DataSetEntry.Location|DropoffLocation_BP|Destination", d),
            Some(1)
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|DropOffLocation_BP|Destination", d),
            Some(1)
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|DropoffLocation|Destination", d),
            Some(1)
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|DropoffLocation2_BP|Destination1", d),
            Some(2)
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|PickupLocation_BP|Location", d),
            None
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|PickupLocation|Location", p),
            Some(1)
        );
        assert_eq!(
            location_slot("DataSetEntry.Location|PickupLocation2_BP|Location1", p),
            Some(2)
        );
    }

    #[test]
    fn objective_kind_from_id() {
        assert_eq!(
            ObjectiveKind::from_id("dropoff_abc_0"),
            ObjectiveKind::Dropoff
        );
        assert_eq!(
            ObjectiveKind::from_id("pickup_abc_0"),
            ObjectiveKind::Pickup
        );
        assert_eq!(ObjectiveKind::from_id("phase_abc"), ObjectiveKind::Phase);
    }
}
