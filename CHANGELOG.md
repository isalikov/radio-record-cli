# Changelog

All notable changes to this project are documented here.

## [Unreleased]

- Add a diagnostics overlay on `d`: player state and generation, codec, sample
  rate, channels, bitrate, the active stream and its fallback round, cache fill,
  worker health, history age, uptime, and a level sparkline.
- Detect a stalled player worker: if an engine that should be alive stops
  reporting for two seconds, the UI shows `Player engine stalled · q quit`
  instead of freezing on a silent "buffering" state.
- Show the real cache percentage next to the buffering dot while a stream fills.
- Split the level line into left and right channel meters with a bold peak-hold
  tick; mono sources mirror the left channel, and the meter still rides the same
  single IPC reply per tick.
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

