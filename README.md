# sc-dossier

Read-only Rust client for Star Citizen account data from CIG's game-services
gRPC backend, authenticated via the RSI launcher's stored session. Currently:
owned blueprints, the entitlement ledger, and stowed inventory items.

Non-official client against CIG's backend — against their ToS. Read-only, own
account, at your own risk.

## How it works

1. Decrypt `launcher store.json` (electron-store: AES-256-CBC + PBKDF2-HMAC-SHA512,
   key read from the launcher's `app.asar`) → `X-Rsi-Token` + `X-Rsi-Device`.
2. Mint a game JWT: `POST games/claims → games/release → games/token` at
   `…/api/launcher/v3`. `games/release` returns the current `servicesEndpoint`.
3. gRPC/TLS to that endpoint → `BlueprintLibraryService.QueryBlueprintEntries`,
   paginated.

Blueprints read on the account JWT directly. The entitlement and entity-graph
services are sharded by *character* geid and reject the account JWT, so
`entitlements()` / `items()` / `inventories()` first swap it for a player-scoped
JWT via `IdentityService.GetCurrentPlayer` (cached after the first call;
`player_id()` exposes the geid once resolved).

## Usage

```toml
[dependencies]
sc-dossier = { git = "https://github.com/VeeLume/sc-dossier", tag = "v0.1.0" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

```rust
use sc_dossier::Dossier;

#[tokio::main]
async fn main() -> Result<(), sc_dossier::Error> {
    let dossier = Dossier::from_launcher("my-app/1.0").await?;
    for bp in dossier.owned_blueprints().await? {
        println!("{} (item_class {})", bp.blueprint_id, bp.item_class_id);
    }
    Ok(())
}
```

The caller supplies the gRPC user-agent. Queries return raw GUIDs
(`blueprint_id` / `category_id` / `item_class_id`); name resolution is the
caller's responsibility.

## API

| Item | Purpose |
|---|---|
| `Dossier::from_launcher(user_agent)` | read store creds → mint → connect |
| `Dossier::from_session(session, user_agent)` | connect with a minted session |
| `Dossier::owned_blueprints()` | full owned set, auto-paginated |
| `Dossier::entitlements()` | account-wide owned-item ledger (streamed) |
| `Dossier::items()` | every stowed item, everywhere, tagged with `Location` |
| `Dossier::inventories()` | inventory containers (incl. empty) + type/capacity |
| `Dossier::player_id()` | current character geid, once resolved |
| `Dossier::session()` | active endpoint / version / JWT |
| `store::read_credentials()` | launcher-store decryption → `Credentials` |
| `auth::{mint_session, Env, MintOptions, Session}` | mint flow |
| `client::{connect, BearerAuth}` | tonic channel (caller supplies the user-agent) |
| `Blueprint`, `Entitlement`, `Item`, `Inventory`, `Error`, `Result` | data + error types |

All ids stay raw/wire-native (CRCs, GUIDs, geids); a holotable consumer resolves
`class_crc` → item, `resource_id` → resource type, and `Context::Location/Hangar`
CRCs → place names via its `by_crc` indices.

## Build

`cargo build` — `protoc` is supplied by `protoc-bin-vendored`; `.proto` files are
compiled by `build.rs`.

## Attribution

Proto definitions, launcher-store format, and mint flow from
[19h/space-reversing](https://github.com/19h/space-reversing).

## License

MIT — see [LICENSE](LICENSE).
