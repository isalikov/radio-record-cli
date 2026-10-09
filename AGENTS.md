# AGENTS.md

Guidance for AI coding agents (Claude Code, Codex, Cursor, Gemini CLI, and others) working in this repository.

## Working rules

These apply to every agent and every session in this repo.

1. **Never change git state.** Read-only git is allowed: `git status`, `git diff`, `git log`, `git show`, `git ls-files`, `git blame`. Everything else (`add`, `commit`, `push`, `checkout`, `stash`, `reset`, ...) is forbidden; the user commits by hand. In Claude Code, `.claude/settings.json` and the `.claude/hooks/block-git.sh` hook enforce this, including inside compound commands. Other agents must follow the rule on their own.
2. **End every session that changed files by suggesting exactly one commit message for everything uncommitted.** Run `git status --short`, `git diff`, and `git diff --cached`, and describe what is actually in the tree (staged, unstaged, and untracked), not what you remember doing. Never propose splitting into several commits. Use the `commit-message` skill (`.claude/skills/commit-message/SKILL.md`): `feat/`, `fix/`, or `chore/` prefix, imperative subject, a short body listing the changes, printed in a single code block for the user to paste. Print the message only, never a `git add` or `git commit` command.
3. **Update README.md after any player change.** If you touched `src/player.rs`, `src/app.rs`, `src/ui.rs`, or `src/main.rs` in a way that changes keybindings, playback, metering, settings, or env options, sync `README.md` and `CHANGELOG.md` in the same session, before the commit message. The `readme-sync` skill describes what to check.

## What this is

radiome is a keyboard-only terminal radio player for [Radio Record](https://www.radiorecord.ru/), written in Rust (edition 2024) with Ratatui + Crossterm. It is a single binary crate (`src/main.rs`) with no workspace and no async runtime. Audio plays through the **native engine**: rustls HTTP (`ureq`) fetches streams, a pure-Rust AAC-LC decoder (`symphonia`) turns ADTS into f32 samples, a fixed-capacity wait-free ring (`rtrb`) feeds a `cpal` output callback (CoreAudio on macOS, ALSA on Linux — the only runtime library). If **mpv** is installed it is used as a fallback engine for HE-AAC streams (`RADIOME_ENGINE=native|mpv` overrides selection); the mpv engine speaks JSON IPC over a private Unix socket. API requests go through `ureq` with rustls and webpki roots. Unit tests need neither network nor audio devices; fixtures of the real streams live in `tests/fixtures/`.

## Commands

The Rust toolchain is pinned in `mise.toml` (`rust = "stable"`). With [mise](https://mise.jdx.dev) activated in the shell, `cargo` resolves automatically inside this directory; run `mise install` once after cloning. mpv is an optional system package and is not managed by mise. Linux source
builds need ALSA development headers and pkg-config.

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

Dev overrides: `RADIOME_BASE_URL` (API base), `RADIOME_CONFIG_DIR` (settings location, precedence over `XDG_CONFIG_HOME`), `RADIOME_ENGINE` (`auto`/`native`/`mpv`), `RADIOME_THEME` (color theme), `NO_COLOR`.

## Architecture

Three threads of control, all communicating through `std::sync::mpsc`; the main thread never blocks on network or audio.

**Main loop (`main.rs`)** — single-threaded event loop ticking every ~10 ms. Each iteration: poll network receivers, pull the latest player `Snapshot` plus diagnostics fields (generation, worker stall flag, uptime, history age), drain mpv-originated media commands (prev/next), record one meter sample into `App::meter_history` under the redraw gate, redraw at most every 50 ms, debounce-save settings (500 ms) when `app.dirty`, then poll one crossterm event. `Network` holds `Option<Receiver>`s for the in-flight catalog and history fetches; each fetch is a one-shot `thread::spawn` + channel (`fetch()`). History refreshes every 15 s while a station is playing.

**`app.rs` — pure state machine.** `App::key(KeyEvent) -> Action` is the only input entry point and returns an `Action` enum (`Play(streams)`, `Pause`, `Stop`, `Volume`, `Refresh`, `Quit`, `None`) that `main.rs` dispatches to the player/network. `App` never touches I/O, which is why keyboard behavior is unit-tested by pressing keys on `app::tests::fixture()` and asserting on `Action`s and state. View state (`help`, `help_scroll`, `show_history`, `show_diag`) and diagnostics display fields (`generation`, `uptime`, `history_age`, `worker_stalled`, `meter_history`) live here by precedent; `main.rs` fills the data fields, and `push_meter_sample` bounds the sparkline history. Key invariants encoded here and in tests: browsing (arrows, Tab) never changes the playing station (`now`); categories are `All`, `Favorites`, then genres derived from the catalog; `set_history` drops results for a station that is no longer `now` (stale-response guard).

**`player.rs` — engine-agnostic core.** `Player` (main-thread handle) sends `Request`s to a worker thread that owns one `Box<dyn EngineImpl>` at a time; `engine::open` reselects native or mpv for each Play (default auto: native when the stream list contains AAC-LC-playable entries and an output device exists). In auto mode, native exhaustion or device failure drops native and tries mpv once with the full station list, current volume, and pause state; explicit overrides disable this transition. Every `Play`/`Stop` bumps a `generation` counter; the worker tags each `Snapshot` it sends back with the generation it belongs to, and `Player::update` discards snapshots from older generations so late events can't resurrect a stopped or replaced station. `Snapshot` is the one bus from engine to UI: state, per-channel `levels` (the single footer bar rides the louder channel; mono mirrors the left), telemetry (codec/samplerate/channels/bitrate), buffering percent, cache seconds, active stream URL and fallback round, underruns. New fields must be `Default + Clone` and reset on play/stop/stream-start. `Player::worker_stalled()` flags a worker that stopped reporting for 2 s while audio should be alive. `Streams` is the engine-independent fallback machine (3 rounds, 2 s apart, names unreachable hosts); `AudioMeter` normalises dB readings (rolling percentiles, attack/release easing) and is fed by either engine. On macOS, `media.rs` owns the system RemoteCommandCenter session for both engines, pumps the main Cocoa run loop, and queues media keys into `App::key`; mpv is launched with its system media-key handler disabled. The mpv IPC PREV/NEXT bindings still flow through the worker for tests and other platforms. Native engine invariants: no locks anywhere (mpsc outside the audio path, SPSC ring + atomics inside it); the output callback never blocks/allocates/logs and converts the stream rate to the device rate with linear interpolation (upsampling only — a device slower than the stream is refused); every blocking operation has a timeout plus a generation check; each fallback URL gets a fresh ring and output stream; the reader waits for ring space with generation and pause checks rather than discarding overflow; the callback owns the pre-gain RMS meter, cache fill, underruns, and 0.3 s prebuffering; pause silences the next callback and clears the ring and resampler even across a quick pause/resume; the reader drains paused audio, and HLS resume rejoins recent segments; HE-AAC-only stream lists are refused up front, never half-played.

**`api.rs` + `json.rs`.** `Client` uses `ureq` (rustls + webpki roots, gzip) with 5 s connect / 15 s global timeouts and decodes with a hand-written JSON parser (`json.rs`) into plain structs (`Catalog`, `Station`, `Track`). `json.rs` rejects input nested deeper than 128 levels to avoid a stack-overflow abort. API decoding intentionally uses `json.rs`; serde_json is reserved for the mpv engine's IPC. `Station::streams()` returns labeled `Stream`s in fallback priority (`Main`/AAC-LC direct, `Hls`, `High`/HE-AAC, `Low`/HE-AACv2, duplicates skipped) so each engine can filter by capability.

**`ui.rs` — render only.** `ui::render(frame, &mut App, &Theme)` reads `App` and draws; it holds no state except the `ListState`s inside `App`. Colors come from a `Theme` resolved once at startup (`RADIOME_THEME`, with `NO_COLOR` taking precedence). Layout gates: below 44×12 it shows a placeholder, history panel is hidden when the body is shorter than 12 rows, sidebar width switches at 90 columns. The footer is four rows: one audio-level bar, a status line (which shows the real buffering percentage and, in diag mode, telemetry segments), the track, and a global error. The `d` diagnostics overlay and the `?` help popup are gated like each other with `saturating_sub` math. UI tests render into `ratatui::backend::TestBackend` and assert on buffer contents.

**`settings.rs`.** `Settings { favorites: BTreeSet<i64>, volume: u8 }` serialized with serde to `settings.json`; saves go through a `NamedTempFile` + `persist` so writes are atomic, and a corrupt file is an error rather than being overwritten.

**`error.rs`.** One string-backed `Error` type and a crate-wide `Result<T>` alias; there is no error enum, messages are meant to be shown to the user directly.

## Conventions worth knowing

- Station and track names keep their original (Russian) spelling from the API; only application labels are English. `Category::title()` translates a fixed set of genre names.
- User-facing status strings follow the pattern `"<Problem> · r retry"` (see `main.rs`).
- Tests live in `#[cfg(test)] mod tests` inside each file; `app::tests` is `pub` so `ui.rs` can reuse `fixture()` and `press()`.
- `.editorconfig`: 4-space Rust, LF, final newline; `Makefile` uses tabs.
- README.md documents the full keyboard map and runtime behavior; update it (and `CHANGELOG.md`) when changing keybindings or user-visible behavior.
