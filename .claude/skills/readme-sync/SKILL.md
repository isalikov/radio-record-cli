---
name: readme-sync
description: Bring README.md (and CHANGELOG.md) in line with player changes. Use after any edit to src/player.rs, src/app.rs, src/ui.rs, or src/main.rs that alters keybindings, playback behavior, audio metering, settings, or CLI/env options.
---

# README sync

README.md is the user manual for radio-record. Every player change must be reflected there in the same session, before suggesting a commit message.

## What to check

Read `README.md` and compare each section against the current code:

| Section | Source of truth |
| --- | --- |
| Run / requirements | `Cargo.toml`, built-in HTTP transport and native audio output in `src/engine/native.rs` |
| Keyboard table | `App::key` in `src/app.rs`, help text in `ui::help` |
| Audio and settings | `AudioMeter`, native output format and resampling, `Settings` fields and path precedence, env vars (`RADIO_RECORD_BASE_URL`, `RADIO_RECORD_CONFIG_DIR`), stream priority in `Station::stream_url`, refresh interval in `main.rs` |
| Verify | `Makefile` targets |

## Steps

1. Identify which user-visible behavior changed. If nothing user-visible changed (pure refactor, test-only), say so and stop.
2. Edit the affected README sections. Keep the existing voice: short declarative sentences, no marketing language, application labels in English.
3. If a keybinding changed, update **both** the README table and the in-app help in `ui::help` so they agree.
4. Add a line under the current unreleased heading in `CHANGELOG.md`. Create an `## [Unreleased]` section above the latest version if none exists.
5. Report which sections you touched. Do not run git.
