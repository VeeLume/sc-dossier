//! Global events (`ScenarioService`): which run now, and the player's share.
//!
//! An event is a `MissionScenario` DCB record; its GUID is the template id the
//! backend takes. The backend cannot list them: the caller supplies the ids
//! (sc-holotable has the records). Progress maps are keyed by `Faction` record
//! GUID. Points shared with the party count toward the overall total only.

use std::collections::BTreeMap;

use tonic::transport::Channel;
use tonic::Code;

use crate::client::BearerAuth;
use crate::error::Result;
use crate::proto::mission::ScenarioStatus;
use crate::proto::mission_scenario::scenario_service_client::ScenarioServiceClient;
use crate::proto::mission_scenario::{
    GetContributorScenarioProgressRequest, QueryScenarioStatusRequest,
};

/// The current state of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EventState {
    /// Disabled.
    Disabled,
    /// Waiting for its next run.
    Scheduled,
    /// Starting up.
    Starting,
    /// Live.
    Running,
    /// Shutting down.
    Stopping,
    /// Ended.
    Stopped,
    /// The backend reports an error.
    Error,
    /// A value this crate doesn't know.
    Unknown(i32),
}

impl EventState {
    fn from_wire(v: i32) -> Self {
        match ScenarioStatus::try_from(v) {
            Ok(ScenarioStatus::Disabled) => Self::Disabled,
            Ok(ScenarioStatus::Scheduled) => Self::Scheduled,
            Ok(ScenarioStatus::Starting) => Self::Starting,
            Ok(ScenarioStatus::Running) => Self::Running,
            Ok(ScenarioStatus::Stopping) => Self::Stopping,
            Ok(ScenarioStatus::Stopped) => Self::Stopped,
            Ok(ScenarioStatus::Error) => Self::Error,
            _ => Self::Unknown(v),
        }
    }
}

/// The state of an event's current run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventStatus {
    /// `MissionScenario` record GUID.
    pub template_id: String,
    /// State of the current run.
    pub state: EventState,
}

/// An event's progress toward its goals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventProgress {
    /// Whether the event is running.
    pub active: bool,
    /// Everyone's points, by `Faction` GUID.
    pub overall: BTreeMap<String, i64>,
    /// The player's own points, by `Faction` GUID. Excludes points shared from
    /// party members.
    pub own: BTreeMap<String, i64>,
}

/// The current run of each event. The backend keeps every past run as a
/// deprecated row; those are dropped. Unknown ids are absent from the result.
pub(crate) async fn status(
    channel: &Channel,
    jwt: &str,
    template_ids: &[String],
) -> Result<Vec<EventStatus>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ScenarioServiceClient::with_interceptor(channel.clone(), auth);
    let request = QueryScenarioStatusRequest {
        template_ids: template_ids.to_vec(),
    };
    let response = client.query_scenario_status(request).await?.into_inner();
    Ok(response
        .results
        .into_iter()
        .filter(|b| b.status != ScenarioStatus::Deprecated as i32)
        .map(|b| EventStatus {
            state: EventState::from_wire(b.status),
            template_id: b.template_id,
        })
        .collect())
}

/// Progress of one event for `contributor` (a player geid). `None` if the event
/// doesn't track progress.
pub(crate) async fn progress(
    channel: &Channel,
    jwt: &str,
    template_id: &str,
    contributor: u64,
) -> Result<Option<EventProgress>> {
    let auth = BearerAuth::new(jwt)?;
    let mut client = ScenarioServiceClient::with_interceptor(channel.clone(), auth);
    let request = GetContributorScenarioProgressRequest {
        template_id: template_id.to_string(),
        contributor_id: contributor,
    };
    match client.get_contributor_scenario_progress(request).await {
        Ok(r) => {
            let r = r.into_inner();
            Ok(Some(EventProgress {
                active: r.is_scenario_active,
                overall: r.overall_progress.into_iter().collect(),
                own: r.contributor_progress.into_iter().collect(),
            }))
        }
        Err(s) if s.code() == Code::FailedPrecondition => Ok(None),
        Err(s) => Err(s.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn states() {
        assert_eq!(EventState::from_wire(4), EventState::Running);
        assert_eq!(EventState::from_wire(6), EventState::Stopped);
        assert_eq!(EventState::from_wire(42), EventState::Unknown(42));
    }
}
