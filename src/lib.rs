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
//! All ids stay wire-native (GUIDs, CRCs, geids); naming them is the caller's
//! job.
//!
//! [`spectrum`] reads RSI's Spectrum chat/forum API on the same launcher
//! session — e.g. the staff feed across forums and chat lobbies.
//!
//! [`patchnotes`] and [`commlinks`] read the LIVE patch notes and the
//! comm-link listing from the public RSI website, anonymously; [`pledge`] the
//! pledge store's ship list.
//!
//! Using a non-official client against CIG's backend is against their Terms of
//! Service. This crate is read-only; use on your own account at your own risk.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod html;
mod identity;
mod proto;

pub mod auth;
pub mod blueprints;
pub mod client;
pub mod commlinks;
pub mod entitlements;
pub mod error;
pub mod items;
pub mod loadouts;
pub mod missions;
pub mod patchnotes;
pub mod pledge;
pub mod reputation;
pub mod spectrum;
pub mod stash;
pub mod store;

pub use blueprints::{Blueprint, BlueprintProcessType, BlueprintSource};
pub use entitlements::{Entitlement, EntitlementSource, EntitlementStatus};
pub use error::{Error, Result};
pub use items::{Context, Inventory, InventoryType, Item, Location, Resource};
pub use loadouts::{Loadout, LoadoutEntry, LoadoutItem, LoadoutSlot};
pub use missions::{Mission, MissionObjective, ObjectiveKind, ObjectiveState};
pub use reputation::{Reputation, Standing};
pub use stash::{
    port_crc, Crafted, Ingredient, IngredientKind, ItemNode, Ownership, Stash, Stashes,
};

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
        Ok(Self {
            channel,
            session,
            player: OnceCell::new(),
        })
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

    /// Fetch the player's accepted missions (contracts) with their objectives
    /// and location CRCs. Location CRCs are wire-native and don't resolve via the
    /// item catalog — name them at the call site (see [`missions`]).
    pub async fn active_missions(&self) -> Result<Vec<Mission>> {
        let player = self.player().await?;
        missions::query(&self.channel, &player.jwt, player.geid).await
    }

    /// Every inventory as item trees (attachments, contents, ownership, crafting).
    pub async fn stashes(&self) -> Result<Stashes> {
        let player = self.player().await?;
        stash::all(&self.channel, &player.jwt, player.geid).await
    }

    /// One inventory as item trees.
    pub async fn stash(&self, location: &Location) -> Result<Stash> {
        let player = self.player().await?;
        stash::one(&self.channel, &player.jwt, location).await
    }

    /// Stock and saved loadouts of every owned item.
    pub async fn loadouts(&self) -> Result<Vec<Loadout>> {
        let player = self.player().await?;
        loadouts::query(&self.channel, &player.jwt).await
    }

    /// The player's reputation standings, one per `(org, scope)`, with the
    /// current rank. Ids are DCB keys; see [`reputation`].
    pub async fn reputations(&self) -> Result<Vec<Reputation>> {
        let player = self.player().await?;
        reputation::query(&self.channel, &player.jwt).await
    }
}
