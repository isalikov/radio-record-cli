# Changelog

All notable changes to this project are documented here.

## [v2.0.0] - 2026-10-09

### Added

- Native AAC-LC playback through Symphonia and CoreAudio/ALSA, including
  direct streams and unencrypted ADTS HLS streams. mpv and curl are no longer
  required at runtime; Linux still requires the ALSA runtime library.
- Automatic engine selection for each station. Native plays AAC-LC; optional
  mpv handles HE-AAC and native-engine failures. `RADIOME_ENGINE=auto|native|mpv`
  selects automatic behavior or forces an engine.
- Application-owned macOS system previous/play-pause/next controls for both
  engines, with station metadata and playback state. Ordinary terminal
  F7/F8/F9 and forwarded media-key events remain supported.
- Diagnostics overlay on `d`: state, generation, codec, sample rate, channels,
  bitrate, active stream and retry round, buffering, cache fill, underruns,
  worker health, history age, uptime, and audio-level history.
- `RADIOME_THEME` color themes: `default`, `amber`, `phosphor`, and `paper`.
  `NO_COLOR` takes precedence; unknown themes fall back to the default.
- Recorded AAC-LC, HE-AAC, and HLS fixtures, offline playback regression tests,
  and optional live network and audio-device checks.
- Uninstall script with optional settings removal through `--purge` or
  `RADIOME_PURGE=1`, plus mise-based Rust setup instructions.

### Changed

- Replace curl API requests with `ureq`, rustls, and bundled webpki roots;
  retain the 5-second connect and 15-second request timeouts.
- Use one smooth audio-level bar following the louder channel before software
  gain. Silence, pause, and buffering settle the bar.
- Native pause silences output and discards queued audio and resampler state.
  Resume buffers fresh audio; HLS rejoins recent segments. mpv retains its
  own pause/cache behavior.
- Buffer 0.3 seconds of native audio at startup and after starvation. Report
  actual buffering progress, cache fill, underruns, and consumed audio levels.
- Resample to faster output-device rates with linear interpolation; matching
  rates pass frames through directly. Unsupported slower devices are refused.
- Document optional mpv installation and warm it up when present. Add Linux
  source-build dependency hints and ALSA headers to release builds.

### Fixed

- Try alternate station streams when a URL fails, retry the full list up to
  three rounds two seconds apart, and name unreachable hosts in the final error.
  Automatic native-to-mpv fallback preserves volume and pause state.
- Feed complete HLS segments with backpressure instead of dropping overflow
  audio. Validate playlists and relative URLs, retry stalled or corrupt streams,
  and reset output buffers, sample rates, and telemetry when changing URLs.
- Preserve macOS hardware media controls after switching to native playback;
  disable mpv's competing system media-key handler.
- Detect a player worker that stops reporting for two seconds during active
  playback and show a stalled-engine error.
- Allow 20 seconds for mpv's IPC socket to appear on a slow first launch and
  provide a useful retry message.
- Reject API JSON nested deeper than 128 levels and cap pending mpv IPC output
  at 1 MiB to avoid stack overflow and unbounded queue growth.

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

