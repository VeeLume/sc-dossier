# Changelog

## v0.12.0

- `Dossier::reputations() -> Vec<Reputation>`: the player's reputation
  standings (`ReputationService.QueryReputations`, player-scoped), one per
  `(org, scope)`. Per `Reputation`: row `id`, `org_geid` (parsed from the
  `entity` urn), `entity`, `scope`, `score`, `scope_id`, `updated_at_ms`, and
  the current `Standing { id, name, min, drift_amount, drift_every_secs,
  gated }`. All ids are DCB keys: `org_geid` = `FactionReputation.GEID`,
  `scope_id` = the `SReputationScopeParams` record with the full rank ladder,
  `Standing::id` = the `SReputationStandingParams` record.
- `examples/reputations_dump.rs`.

## v0.11.0

- `pledge`: the pledge store's ship list from the public RSI website,
  anonymous. `PledgeStore::ships() -> Vec<StoreShip>` walks the store
  GraphQL (30 per page, about 9 calls), newest store page first. Per
  `StoreShip`: ship-matrix `id`, `name`, store `url`, `msrp_cents` (USD),
  `published_at`, and `media` (thumbnail, slideshow, 1920×1080 wallpaper).
  `published_at` is when the store page went up, roughly the first concept
  sale, **not** the in-game release; early rows share a backfill date and a
  few have none.

## v0.10.0

- `auth::library(creds, opts) -> Vec<Game>`: the games and release channels
  this account may play (`games/claims` → `games/library`). Each
  `GameChannel` has id, name, `version` (client changelist), `version_label`,
  `platform_id`, `services_endpoint`, `install_dir`, `weight`, and `network`.
  Account-scoped: a test channel shows only while it is open to this account.
- `Spectrum::hotfix_central() -> Option<HotfixCentral>`: the pinned Hotfix
  Central thread in SC's Patch Notes forum, anonymous, two calls. Includes the
  opening post's last edit time (`edited_ms`), the status block (version,
  date, `hotfix_channel_open`, LIVE client and DGS changelists), the hotfix
  entries newest first (date, headline, builds, channel, items), and the raw
  text.
- `Error::Mint` now also covers `games/library`; its message says "launcher
  step".

## v0.9.0

- `commlinks`: `CommLinks::videos(url) -> Vec<Video { host, id }>`, the
  videos a comm-link embeds (YouTube; Vimeo on a few 2013 posts), with
  `Video::watch_url()`. Reads the comm-link page and its Alexandria fragment;
  the listing carries no video ids. Covers the three page markups seen from
  2013 to now.

## v0.8.0

- `spectrum`: what a staff post answers.
  - `Spectrum::reply(id) -> Reply { id, thread_id, parent_reply_id,
    root_reply_id, member, text, highlight_role_id, is_erased, time_ms }`
    (`/v2/forum/thread/reply/get`); a `None` parent means the reply answers
    the thread opener.
  - `Spectrum::thread_opener(slug) -> ThreadOpener`
    (`/v2/forum/thread/get`).
  - Both work anonymously.
- `Message::mentions: Vec<Mention { member_id, nickname }>` from the chat
  message's `MENTION` entities. Chat has no reply-to relation.
- `lobby_history`'s `before` cursor is documented as inclusive.
- `Member` and `Message` now derive `PartialEq`, `Serialize`, `Deserialize`;
  `Member::is_gm` also reads the v2 forum's `isGm`.

## v0.7.0

- `commlinks`: the comm-link listing from the public RSI website, anonymous.
  `CommLinks::page(&CommLinkQuery)` → `CommLinkPage { items, is_last }`, 10
  per page, filterable by channel, series, kind, and text, newest or oldest
  first. Per `CommLink`: id (from the URL), url, title, kind, channel, text
  summary, thumbnail, comment count, and the relative age as shown plus a
  parsed coarse `Age` — the listing has no absolute dates.

## v0.6.0

- `patchnotes`: LIVE patch notes from the public RSI website, anonymous.
  `PatchNotes::index()` (`/en/patch-notes`, newest first) and
  `PatchNotes::article(url)` → `PatchNoteArticle { releases }`. Per `Release`:
  date, version, channel, patch name, build (`VERSION` line), build info,
  one-line highlights, item counts, and the content as text-only
  `Section`/`Block`s. Covers both article templates (≤ 4.10.1 FAQ, 4.10.2+
  MiniGrid/ArtemisNarrative) and articles that accumulate point releases.
- `Error::Page` for unexpected web-page responses.
- New dependency: `chrono` (no default features, `serde`).
