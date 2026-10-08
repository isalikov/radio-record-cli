# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex, Cursor, Gemini CLI, and others) working in this repository.

## Working rules

These apply to every agent and every session in this repo.

1. **Never change git state.** Read-only git is allowed: `git status`, `git diff`, `git log`, `git show`, `git ls-files`, `git blame`. Everything else (`add`, `commit`, `push`, `checkout`, `stash`, `reset`, ...) is forbidden; the user commits by hand. In Claude Code, `.claude/settings.json` and the `.claude/hooks/block-git.sh` hook enforce this, including inside compound commands. Other agents must follow the rule on their own.
2. **End every session that changed files by suggesting exactly one commit message for everything uncommitted.** Run `git status --short`, `git diff`, and `git diff --cached`, and describe what is actually in the tree (staged, unstaged, and untracked), not what you remember doing. Never propose splitting into several commits. Use the `commit-message` skill (`.claude/skills/commit-message/SKILL.md`): `feat/`, `fix/`, or `chore/` prefix, imperative subject, a short body listing the changes, printed in a single code block for the user to paste. Print the message only, never a `git add` or `git commit` command.
3. **Update README.md after any player change.** If you touched `src/player.rs`, `src/app.rs`, `src/ui.rs`, or `src/main.rs` in a way that changes keybindings, playback, metering, settings, or env options, sync `README.md` and `CHANGELOG.md` in the same session, before the commit message. The `readme-sync` skill describes what to check.

## What this is

radiome is a keyboard-only terminal radio player for [Radio Record](https://www.radiorecord.ru/), written in Rust (edition 2024) with Ratatui + Crossterm. It is a single binary crate (`src/main.rs`) with no workspace, no async runtime, and no HTTP or audio libraries: **curl** (subprocess) fetches API JSON and **mpv** (subprocess, JSON IPC over a Unix socket) plays audio. Both must be installed to run the app; unit tests do not need them.

## Commands

The Rust toolchain is pinned in `mise.toml` (`rust = "stable"`). With [mise](https://mise.jdx.dev) activated in the shell, `cargo` resolves automatically inside this directory; run `mise install` once after cloning. mpv and curl are system packages and are not managed by mise.

```sh
make build        # cargo build
make run          # cargo run (must be run in a real terminal; stdin/stdout are checked)
make test         # cargo test (all non-ignored tests, no mpv/network needed)
make check        # cargo fmt --check && cargo clippy --all-targets -- -D warnings
make fmt          # cargo fmt
make test-audio   # runs the one #[ignore]d test that spawns real mpv with --ao=null
```

Run a single test by name, e.g. `cargo test parses_history` or `cargo test app::tests::`. Clippy warnings are errors in `make check`, so keep the tree clippy-clean.

Releases are built by `.github/workflows/release.yml` on `v*` tags with `cargo build --release --locked`, so `Cargo.lock` must be committed and up to date.

Dev overrides: `RADIOME_BASE_URL` (API base), `RADIOME_CONFIG_DIR` (settings location, precedence over `XDG_CONFIG_HOME`), `RADIOME_THEME` (color theme), `NO_COLOR`.

## Architecture

Three threads of control, all communicating through `std::sync::mpsc`; the main thread never blocks on network or audio.

**Main loop (`main.rs`)** — single-threaded event loop ticking every ~10 ms. Each iteration: poll network receivers, pull the latest player `Snapshot` plus diagnostics fields (generation, worker stall flag, uptime, history age), drain mpv-originated media commands (prev/next), record one meter sample into `App::meter_history` under the redraw gate, redraw at most every 50 ms, debounce-save settings (500 ms) when `app.dirty`, then poll one crossterm event. `Network` holds `Option<Receiver>`s for the in-flight catalog and history fetches; each fetch is a one-shot `thread::spawn` + channel (`fetch()`). History refreshes every 15 s while a station is playing.

**`app.rs` — pure state machine.** `App::key(KeyEvent) -> Action` is the only input entry point and returns an `Action` enum (`Play(url)`, `Pause`, `Stop`, `Volume`, `Refresh`, `Quit`, `None`) that `main.rs` dispatches to the player/network. `App` never touches I/O, which is why keyboard behavior is unit-tested by pressing keys on `app::tests::fixture()` and asserting on `Action`s and state. View state (`help`, `help_scroll`, `show_history`, `show_diag`) and diagnostics display fields (`generation`, `uptime`, `history_age`, `worker_stalled`, `meter_history`) live here by precedent; `main.rs` fills the data fields, and `push_meter_sample` bounds the sparkline history. Key invariants encoded here and in tests: browsing (arrows, Tab) never changes the playing station (`now`); categories are `All`, `Favorites`, then genres derived from the catalog; `set_history` drops results for a station that is no longer `now` (stale-response guard).

**`player.rs` — mpv worker.** `Player` (main-thread handle) sends `Request`s to a worker thread that owns an `Engine` (the mpv `Child` + Unix socket + line-buffered IPC). Every `Play`/`Stop` bumps a `generation` counter; the worker tags each `Snapshot` it sends back with the generation it belongs to, and `Player::update` discards snapshots from older generations so late mpv events can't resurrect a stopped or replaced station. The mpv `input.conf` written per-session maps macOS RemoteCommandCenter PREV/NEXT to `script-message radiome-previous/next`, which the engine turns into `media` commands that flow back to `App::skip_station` — Rust owns the station list; mpv only owns play/pause. The meter lines come from an `astats` lavfi filter polled at 20 Hz; `AudioMeter` normalises dB per channel against a rolling percentile window (mono sources mirror the left channel, and a slowly decaying `peak` rides the same reply), so stereo costs no extra IPC. Telemetry rides the same snapshot: stable properties (`audio-codec-name`, `audio-params/samplerate`, `audio-params/channel-count`, `audio-bitrate`) arrive via `observe_property`, volatile ones (`cache-buffering-state` at 2 Hz while buffering, `demuxer-cache-duration` at 1 Hz while playing) via tick-gated `get_property` with request ids 11 and 12, and the active stream URL plus fallback round come from `Streams` itself. All telemetry fields reset on `play`, `stop`, and `start-file`. `Player::worker_stalled()` flags an engine that stopped reporting for 2 s while audio should be alive; the pending request queue is capped at 1 MiB, mirroring the response cap.

**`api.rs` + `json.rs`.** `Client` shells out to curl and decodes with a hand-written JSON parser (`json.rs`) into plain structs (`Catalog`, `Station`, `Track`). `serde_json` is in the dependency tree but is only used for mpv IPC (`player.rs`) and `settings.rs`; API decoding intentionally uses `json.rs`. Stream selection priority lives in `Station::stream_url()`: `stream_320` → `stream_hls` → `stream_128` → `stream_64`.

**`ui.rs` — render only.** `ui::render(frame, &mut App, &Theme)` reads `App` and draws; it holds no state except the `ListState`s inside `App`. Colors come from a `Theme` resolved once at startup (`RADIOME_THEME`, with `NO_COLOR` taking precedence). Layout gates: below 44×12 it shows a placeholder, history panel is hidden when the body is shorter than 12 rows, sidebar width switches at 90 columns. The footer is five rows: two per-channel meter lines, a status line (which shows the real buffering percentage and, in diag mode, telemetry segments), the track, and a global error. The `d` diagnostics overlay and the `?` help popup are gated like each other with `saturating_sub` math. UI tests render into `ratatui::backend::TestBackend` and assert on buffer contents.

**`settings.rs`.** `Settings { favorites: BTreeSet<i64>, volume: u8 }` serialized with serde to `settings.json`; saves go through a `NamedTempFile` + `persist` so writes are atomic, and a corrupt file is an error rather than being overwritten.

**`error.rs`.** One string-backed `Error` type and a crate-wide `Result<T>` alias; there is no error enum, messages are meant to be shown to the user directly.

## Conventions worth knowing

- Station and track names keep their original (Russian) spelling from the API; only application labels are English. `Category::title()` translates a fixed set of genre names.
- User-facing status strings follow the pattern `"<Problem> · r retry"` (see `main.rs`).
- Tests live in `#[cfg(test)] mod tests` inside each file; `app::tests` is `pub` so `ui.rs` can reuse `fixture()` and `press()`.
- `.editorconfig`: 4-space Rust, LF, final newline; `Makefile` uses tabs.
- README.md documents the full keyboard map and runtime behavior; update it (and `CHANGELOG.md`) when changing keybindings or user-visible behavior.
