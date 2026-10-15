# Changelog

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
