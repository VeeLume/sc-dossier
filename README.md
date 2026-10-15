# sc-dossier

Read-only Rust client for Star Citizen account data from CIG's game-services
gRPC backend, plus RSI's Spectrum chat/forum API, authenticated via the RSI
launcher's stored session. LIVE patch notes from the public RSI website,
anonymously.

Non-official client against CIG's backends — against their ToS. Read-only, own
account, at your own risk.

```toml
[dependencies]
sc-dossier = { git = "https://github.com/VeeLume/sc-dossier", tag = "<release tag>" }
```

API and usage: `cargo doc --open`. `protoc` is vendored; `cargo build` needs
nothing else.

## Attribution

Proto definitions, launcher-store format, and mint flow from
[19h/space-reversing](https://github.com/19h/space-reversing).

## License

MIT — see [LICENSE](LICENSE).
