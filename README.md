# sc-dossier

Read-only Rust client for Star Citizen account data from CIG's game-services
gRPC backend, authenticated via the RSI launcher's stored session. Currently:
owned blueprints.

Non-official client against CIG's backend — against their ToS. Read-only, own
account, at your own risk.

## How it works

1. Decrypt `launcher store.json` (electron-store: AES-256-CBC + PBKDF2-HMAC-SHA512,
   key read from the launcher's `app.asar`) → `X-Rsi-Token` + `X-Rsi-Device`.
2. Mint a game JWT: `POST games/claims → games/release → games/token` at
   `…/api/launcher/v3`. `games/release` returns the current `servicesEndpoint`.
3. gRPC/TLS to that endpoint → `BlueprintLibraryService.QueryBlueprintEntries`,
   paginated.

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
| `Dossier::session()` | active endpoint / version / JWT |
| `store::read_credentials()` | launcher-store decryption → `Credentials` |
| `auth::{mint_session, Env, MintOptions, Session}` | mint flow |
| `client::{connect, BearerAuth}` | tonic channel (caller supplies the user-agent) |
| `Blueprint`, `Error`, `Result` | data + error types |

## Build

`cargo build` — `protoc` is supplied by `protoc-bin-vendored`; `.proto` files are
compiled by `build.rs`.

## Attribution

Proto definitions, launcher-store format, and mint flow from
[19h/space-reversing](https://github.com/19h/space-reversing).

## License

MIT — see [LICENSE](LICENSE).
