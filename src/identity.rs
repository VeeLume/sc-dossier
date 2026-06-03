//! Player resolution. The entity-graph and entitlement services are sharded by
//! *character* geid and reject the account JWT (`"No player Id was found in
//! metadata"`); [`current_player`] swaps it for a player-scoped JWT (+ geid) via
//! `IdentityService.GetCurrentPlayer`. [`crate::Dossier`] caches the result.

use tonic::transport::Channel;

use crate::client::BearerAuth;
use crate::error::{Error, Result};
use crate::proto::identity::identity_service_client::IdentityServiceClient;
use crate::proto::identity::GetCurrentPlayerRequest;

/// The active character's geid and its player-scoped JWT. Cached on first use.
#[derive(Clone)]
pub(crate) struct PlayerSession {
    /// Player-scoped game JWT. Secret.
    pub jwt: String,
    /// The active character's geid — the sharding key.
    pub geid: u64,
}

/// Resolve the account's current character via `GetCurrentPlayer`. Returns
/// [`Error::NoCurrentPlayer`] if there is no player record or an empty JWT.
pub(crate) async fn current_player(channel: &Channel, account_jwt: &str) -> Result<PlayerSession> {
    let auth = BearerAuth::new(account_jwt)?;
    let mut client = IdentityServiceClient::with_interceptor(channel.clone(), auth);

    let response = client.get_current_player(GetCurrentPlayerRequest {}).await?.into_inner();

    let geid = response.player.map(|p| p.geid).filter(|g| *g != 0);
    match (geid, response.jwt) {
        (Some(geid), jwt) if !jwt.is_empty() => Ok(PlayerSession { jwt, geid }),
        _ => Err(Error::NoCurrentPlayer),
    }
}
