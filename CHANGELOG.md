# Changelog

All notable changes to this project are documented here.

## [Unreleased]

- Restore macOS hardware previous/play-pause/next controls with an application
  media session shared by native and mpv playback. Publish the station and
  playback state, pump Cocoa events alongside the terminal loop, and disable
  mpv's competing system media-key handler.

- Add a native audio engine without mandatory mpv/curl processes: rustls HTTP
  feeds a pure-Rust AAC-LC decoder (Symphonia), a wait-free ring buffers samples for
  a CoreAudio/ALSA output callback, and the meter is computed from decoded
  samples directly. mpv and curl are no longer required to run the player.
- Pick the engine automatically: the native engine plays the direct AAC and
  HLS AAC-LC variants; mpv covers HE-AAC streams and native-engine failures.
  Reselect the engine for each station and preserve controls during fallback.
  `RADIOME_ENGINE=native|mpv` forces a choice.
- Feed complete HLS segments with reader backpressure instead of dropping
  overflow audio. Validate playlists, resolve relative URLs, and retry stalled
  playlists or repeated corrupt segments. Reset output and telemetry per URL.
- Install ALSA headers and pkg-config for Linux release builds and document
  Linux source-build dependencies.
- Add a hand-written HLS client (master-variant selection, media playlists,
  live-edge join, ID3-stripping segments) — Record's segments are plain ADTS.
- Replace the curl subprocess with `ureq` (rustls + webpki roots) for API
  requests; timeouts are unchanged (5 s connect, 15 s per request).
- Native pause mutes the next output callback, discards queued audio and
  resampler state, and resumes with fresh audio. HLS rejoins recent segments;
  mpv keeps its own pause/cache behavior.
- Ship recorded stream fixtures (LC, HE-AAC, HLS playlists and segments) and
  live `#[ignore]` tests for the direct stream, HLS, and the audio device.
- Add a diagnostics overlay on `d`: player state and generation, codec, sample
  rate, channels, bitrate, the active stream and its fallback round, cache fill,
  worker health, history age, uptime, and a level sparkline.
- Detect a stalled player worker: if an engine that should be alive stops
  reporting for two seconds, the UI shows `Player engine stalled · q quit`
  instead of freezing on a silent "buffering" state.
- Report native buffering progress toward the 0.3 s startup threshold; output
  consumption updates cache fill, underruns, and the pre-gain meter.
- Simplify the audio meter to one smooth horizontal bar riding the louder
  channel; the per-channel peak-hold tick is gone. Silence, pause, and
  buffering settle the bar.
- Resample up to the output device's rate using linear interpolation; matching
  rates pass every frame through without look-ahead.
- Reject JSON nested deeper than 128 levels in the API parser instead of
  overflowing the stack, which would abort without restoring the terminal.
- Cap the pending mpv IPC request queue at 1 MiB, mirroring the response cap, so
  an mpv that stops reading its socket is reported instead of growing forever.
- Add color themes selected with `RADIOME_THEME`: `amber` (monochrome amber CRT),
  `phosphor` (green terminal), and `paper` (light), alongside `default`. Unknown
  names fall back to `default` with a note on stderr.
- Respect `NO_COLOR`, which was documented but not implemented: colors fall back
  to terminal defaults, and `NO_COLOR` takes precedence over `RADIOME_THEME`.
- Fall back to the station's next stream (`stream_320` → `stream_hls` →
  `stream_128` → `stream_64`) when one fails to load, instead of stopping with
  `Stream unavailable: loading failed`. Fixes playback when the direct stream
  host is unreachable but HLS works (e.g. behind a split-tunnel VPN).
- When every stream of a station fails, retry the whole list up to two more times,
  2 seconds apart, so rotating DNS pools get a chance to return a reachable address.
- The final stream error names the hosts that could not be reached
  (`<host> unreachable · Enter to retry`) instead of `loading failed`.
- Wait up to 20 seconds (was 5) for mpv to open its IPC socket, so a slow first
  launch of a freshly installed mpv on macOS no longer fails with
  `mpv IPC: No such file or directory`. The error now says how to retry.
- Installer checks for mpv and curl after installing and prints the install
  command when missing; warms up mpv once so the first station plays promptly.
- README: troubleshooting section for the two mpv startup errors; mise-based
  Rust setup instructions.
- `scripts/uninstall.sh` removes the installed binary and, with `--purge` or
  `RADIOME_PURGE=1`, the settings directory. Documented in the README.

## [v1.0.0] - 2026-09-07

Initial release of the keyboard-only Rust radio player.

- Browse Radio Record stations by category.
- Play, pause, stop, and switch between stations with the keyboard.
- Support F7/F8/F9 transport controls and forwarded media-key events.
- Manage favorites with persistent local settings.
- Adjust player volume independently from system volume.
- Show recent tracks and current playback metadata.
- Display a compact, audio-reactive level line.
- Provide macOS and Linux installation through the release installer.
- Include `make` targets for running, building, testing, checking, and audio integration tests.

