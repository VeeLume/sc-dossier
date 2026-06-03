//! Read-only access to your own Star Citizen account data via CIG's
//! game-services gRPC backend, authenticated with the RSI launcher's stored
//! session.
//!
//! ```no_run
//! # async fn run() -> Result<(), sc_dossier::Error> {
//! let dossier = sc_dossier::Dossier::from_launcher("my-app/1.0").await?;
//! let blueprints = dossier.owned_blueprints().await?;
//! # Ok(()) }
//! ```
//!
//! Using a non-official client against CIG's backend is against their Terms of
//! Service. This crate is read-only; use on your own account at your own risk.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod identity;
mod proto;

pub mod auth;
pub mod blueprints;
pub mod client;
pub mod entitlements;
pub mod error;
pub mod items;
pub mod store;

pub use blueprints::{Blueprint, BlueprintProcessType, BlueprintSource};
pub use entitlements::{Entitlement, EntitlementSource, EntitlementStatus};
pub use error::{Error, Result};
pub use items::{Context, Inventory, InventoryType, Item, Location, Resource};

use tokio::sync::OnceCell;
use tonic::transport::Channel;

/// A connected, authenticated session against the game-services backend.
pub struct Dossier {
    channel: Channel,
    session: auth::Session,
    /// Player-scoped session (geid + JWT), resolved lazily for the
    /// character-sharded queries. See [`Dossier::player`].
    player: OnceCell<identity::PlayerSession>,
}

impl Dossier {
    /// Read launcher credentials, mint a game token, and connect. `user_agent`
    /// is the gRPC user-agent for backend calls. Returns [`Error::Mint`] if the
    /// stored session is stale.
    pub async fn from_launcher(user_agent: &str) -> Result<Self> {
        let creds = store::read_credentials()?;
        let opts = auth::MintOptions::from_credentials(&creds);
        let session = auth::mint_session(&creds, &opts).await?;
        Self::from_session(session, user_agent).await
    }

    /// Connect using an already-minted [`auth::Session`].
    pub async fn from_session(session: auth::Session, user_agent: &str) -> Result<Self> {
        let channel = client::connect(&session.services_endpoint, user_agent).await?;
        Ok(Self { channel, session, player: OnceCell::new() })
    }

    /// The active session.
    pub fn session(&self) -> &auth::Session {
        &self.session
    }

    /// Fetch every owned blueprint, walking pagination to completion.
    pub async fn owned_blueprints(&self) -> Result<Vec<Blueprint>> {
        blueprints::owned(&self.channel, &self.session.jwt).await
    }

    /// Resolve and cache the player-scoped session — the account JWT can't read
    /// the character-sharded services. Returns [`Error::NoCurrentPlayer`] if the
    /// account has no active character.
    async fn player(&self) -> Result<&identity::PlayerSession> {
        self.player
            .get_or_try_init(|| identity::current_player(&self.channel, &self.session.jwt))
            .await
    }

    /// The current character's geid, if the player session has been resolved
    /// (by a prior `entitlements`/`items`/`inventories` call). `None` otherwise.
    pub fn player_id(&self) -> Option<u64> {
        self.player.get().map(|p| p.geid)
    }

    /// Fetch the account-wide entitlement ledger (web hangar + in-game grants).
    pub async fn entitlements(&self) -> Result<Vec<Entitlement>> {
        let player = self.player().await?;
        entitlements::query(&self.channel, &player.jwt).await
    }

    /// Fetch every stowed item across all of the player's inventories, each
    /// tagged with its [`Location`].
    pub async fn items(&self) -> Result<Vec<Item>> {
        let player = self.player().await?;
        items::query(&self.channel, &player.jwt, player.geid).await
    }

    /// Fetch the player's inventory containers (including empty ones) with type
    /// and capacity. Secondary to [`Dossier::items`].
    pub async fn inventories(&self) -> Result<Vec<Inventory>> {
        let player = self.player().await?;
        items::inventories(&self.channel, &player.jwt, player.geid).await
    }
}
