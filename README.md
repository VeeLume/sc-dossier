# sc-dossier

Read-only Rust client for Star Citizen account data from CIG's game-services
gRPC backend, authenticated via the RSI launcher's stored session.

Non-official client against CIG's backend — against their ToS. Read-only, own
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
