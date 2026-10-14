# Changelog

All notable changes to this project are documented here.

## [v3.3.1] - 2026-10-14

### Fixed

- `sharp` covers fill the whole cover area like the other modes: the picture
  is center-cropped to the area's pixel shape and scaled up, instead of fitted
  square with a gap at the side.

## [v3.3.0] - 2026-10-13

### Added

- Cover detail modes on `c`: `off`, `chunky` (7 × 7 blocks), `pixel`
  (half-blocks, the default), `fine` (two-color quadrant blocks, 28 × 14), and
  `sharp` (real pictures). The mode is saved as `"cover"` in `settings.json`;
  `off` skips the download and gives the cover's room to the details.

### Changed

- `RADIO_RECORD_COVER` accepts every mode and overrides the saved one for a
  session. `sharp` asks the terminal for a picture protocol the first time it
  is on, so it can now be switched on without a restart.

## [v3.2.0] - 2026-10-11

### Added

- Liked songs: `l` saves or removes the song on air, with a `♥ SAVED TO VAULT`
  toast in the now-playing border, a `♥` before liked titles, and a liked-song
  count in the header. Songs are stored locally in `settings.json`.
- Track vault on `v`: liked songs in place of the station browser, with
  station and save time, filter (`/`), sort (`o`), and confirmed delete (`x`).
- Link card on Enter: YouTube Music, Spotify, Yandex Music, and Apple Music
  links plus the radiorecord.ru track page; open one (Enter or `1`–`5`), open
  all (`a`), or copy through OSC 52 (`c`). Apple Music and the track page come
  from the API; Yandex Music is upgraded to a direct link by a background
  lookup when artist and title match.

### Changed

- README: new screenshot, quick start, feature overview, and a configuration
  table; build and troubleshooting notes moved to their own sections.

## [v3.1.0] - 2026-10-11

### Added

- Spectrum visualizer: 48 log-spaced bands drawn as stacked `▬` pill columns,
  computed from the heard audio with a 2048-point FFT (`realfft`) on the worker
  thread, compensated for output latency, with auto-gain and gravity release.
  The audio callback publishes samples through a lock-free ring of atomics.
- Now-playing panel in windows at least 24 rows tall: station-named title bar
  with volume, half-block pixel-art cover, title and artist, estimated track
  progress, and previous/play-pause/next buttons.
- `RADIO_RECORD_COVER=sharp` shows real cover images through the kitty, sixel,
  or iTerm2 protocols (`ratatui-image`), falling back to pixel art.

- Cyberpunk HUD: status line with playback state, favorite count, and uptime;
  heavy notched frames for SECTOR, CHANNELS (with position count and a
  `◉ LIVE` tag), and INTERCEPT LOG; a NOW PLAYING frame with `[LIVE]`, a
  volume meter, status, and a buffer signal meter in its borders, plus a
  metadata line and always-on stream format.
- NEON-77 default palette with `warn` and `glow` tokens in every theme.
- `RADIO_RECORD_FX=full|calm|off`: bass-reactive frame glow, peak caps, and a
  `SIGNAL ACQUIRING` buffering fill; in `full`, a title decode on track change
  and an ON AIR pulse. `NO_COLOR` caps it at `calm`.

### Changed

- The visualizer replaces the single audio-level bar in the footer; the level
  remains in the diagnostics sparkline.
- The visualizer now uses 96 bands from a 4096-point FFT, per-band adaptive
  scaling, no repeated columns, a cyan-to-magenta gradient, and falling peak
  caps, so mids and highs move as much as the bass.
- The now-playing panel drops its blank rows and puts the visualizer beside
  the cover; the pause symbol is `▮▮` everywhere.

## [v3.0.0] - 2026-10-09

### Added

- Radio Record account window on `a`: email/password sign-in, masked input and
  paste support, profile and Premium status, manual sync, and confirmed sign-out.
  The sidebar shows the profile email while signed in.
- Background favorite-station sync on startup, after local edits, and every
  30 seconds. First-time differences and changes on both sides open a merge,
  local-copy, or server-copy dialog with a preview of additions and removals.
- Per-account sync baselines that preserve independent additions and honor
  deletions, private session storage without passwords, server revalidation,
  and preservation of local edits made during requests.
- Offline account API and sync regression tests using a local HTTP server.

### Changed

- Adjust internal player volume in 2% steps with `-` / `=`.

- Rename the project and binary to `radio-record`, move the repository to
  `isalikov/radio-record-cli`, and update release archives and install scripts.
  Configuration uses `~/.config/radio-record` and `RADIO_RECORD_*` overrides.

- Use the built-in audio engine exclusively. Remove external-engine selection,
  IPC transport, installation hints, and fallback-specific tests.
- Make `make test-audio` open the real output device silently and verify its
  format stays unchanged.

### Fixed

- Preserve the current hardware rate during Bluetooth calls, including 24 kHz
  AirPods output, instead of trying to raise it to the stream rate. Read the
  CoreAudio hardware clock separately from the virtual stream format.
- Add filtered downsampling and mono output mixing; clear filter history on
  pause without allocating in the real-time callback.
- Clear the stalled-worker warning after reporting resumes and start a fresh
  heartbeat deadline on each play request.

## [v2.0.1] - 2026-10-09

### Fixed

- Keep native playback running after device underrun/overrun notifications or
  denied real-time scheduling instead of reporting a fatal audio-device error
  during playback. Fatal output errors still stop playback.

## [v2.0.0] - 2026-10-09

### Added

- Native AAC-LC playback through Symphonia and CoreAudio/ALSA, including
  direct streams and unencrypted ADTS HLS streams. Audio and HTTP transport
  are built in; Linux still requires the ALSA runtime library.
- Application-owned macOS system previous/play-pause/next controls for
  playback, with station metadata and playback state. Ordinary terminal
  F7/F8/F9 and forwarded media-key events remain supported.
- Diagnostics overlay on `d`: state, generation, codec, sample rate, channels,
  bitrate, active stream and retry round, buffering, cache fill, underruns,
  worker health, history age, uptime, and audio-level history.
- `RADIO_RECORD_THEME` color themes: `default`, `amber`, `phosphor`, and `paper`.
  `NO_COLOR` takes precedence; unknown themes fall back to the default.
- Recorded AAC-LC, HE-AAC, and HLS fixtures, offline playback regression tests,
  and optional live network and audio-device checks.
- Uninstall script with optional settings removal through `--purge` or
  `RADIO_RECORD_PURGE=1`, plus mise-based Rust setup instructions.

### Changed

- Replace curl API requests with `ureq`, rustls, and bundled webpki roots;
  retain the 5-second connect and 15-second request timeouts.
- Use one smooth audio-level bar following the louder channel before software
  gain. Silence, pause, and buffering settle the bar.
- Native pause silences output and discards queued audio and resampler state.
  Resume buffers fresh audio; HLS rejoins recent segments.
- Buffer 0.3 seconds of native audio at startup and after starvation. Report
  actual buffering progress, cache fill, underruns, and consumed audio levels.
- Resample to faster output-device rates with linear interpolation; matching
  rates pass frames through directly. Unsupported slower devices are refused.
- Add Linux source-build dependency hints and ALSA headers to release builds.

### Fixed

- Try alternate station streams when a URL fails, retry the full list up to
  three rounds two seconds apart, and name unreachable hosts in the final error.
- Feed complete HLS segments with backpressure instead of dropping overflow
  audio. Validate playlists and relative URLs, retry stalled or corrupt streams,
  and reset output buffers, sample rates, and telemetry when changing URLs.
- Preserve macOS hardware media controls during native playback.
- Detect a player worker that stops reporting for two seconds during active
  playback and show a stalled-engine error.
- Reject API JSON nested deeper than 128 levels to avoid stack overflow.

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

