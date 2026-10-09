# radiome

<p align="center">
  <strong>A keyboard-first internet radio player for macOS and Linux.</strong><br>
  Browse stations, control playback, and keep the music in focus.
</p>

<p align="center">
  <a href="https://github.com/isalikov/radiome/releases"><img src="https://img.shields.io/github/v/release/isalikov/radiome?style=flat-square&color=ff2bd6" alt="Latest release"></a>
  <a href="https://github.com/isalikov/radiome/actions/workflows/release.yml"><img src="https://img.shields.io/github/actions/workflow/status/isalikov/radiome/release.yml?style=flat-square&label=release" alt="Release build"></a>
</p>

<p align="center">
  <img src="assets/player-preview.png" alt="radiome player preview">
</p>

<p align="center">
  <a href="#install">Install</a> ·
  <a href="#keyboard">Keyboard</a> ·
  <a href="#support">Support</a>
</p>

Keyboard-only Rust radio player for [Radio Record](https://www.radiorecord.ru/).
Categories on the left, stations on the right, recent tracks below.
The accent color marks playback; the alert color marks the selected category
and favorites.

## Run

Building from source requires Rust/Cargo. On Linux, also install the ALSA
headers and pkg-config (`sudo apt install libasound2-dev pkg-config` on
Debian/Ubuntu). Release binaries need only the ALSA runtime library
(`libasound.so.2`); macOS uses the built-in CoreAudio framework. HTTP, TLS,
and AAC decoding are compiled into the binary.

The default engine is `auto`: native plays direct AAC-LC and the AAC-LC
variant of HLS. Engine selection runs again for each station. HE-AAC-only
stations use mpv; if native exhausts its streams or the output device fails,
radiome tries mpv once with the station's full stream list, keeping volume
and pause. If mpv is unavailable, the error retains the native failure.
`RADIOME_ENGINE=native|mpv` forces an engine and disables automatic switching.

The Rust toolchain is pinned in `mise.toml`. With [mise](https://mise.jdx.dev) installed,
`mise install` in this directory fetches the right `cargo`, `rustfmt`, and `clippy`.
Any other Rust installation (rustup, distro package) works as well.

```sh
mise install
make run
```

Plain `make` prints help. Minimum terminal size: 44 × 12. History is hidden in
short windows to leave room for stations. Truecolor terminals show the full palette.

`RADIOME_THEME` selects a color theme: `default` (cyan and magenta on dark),
`amber` (monochrome amber CRT), `phosphor` (green terminal), or `paper`
(light, for white terminals). An unknown name falls back to `default` with a
note on stderr. `NO_COLOR` disables colors and takes precedence over the theme.

## Install

Once release archives are published, users can install with one command:

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radiome/master/scripts/install.sh | sh
```

The script downloads the latest GitHub Release for the current macOS or Linux
architecture. If no release archive exists yet, it falls back to `cargo install`
from this repository, so the same command also works for local development
machines that already have Rust installed.

Installed binaries go to `~/.local/bin/radiome` by default. If that directory is
not in `PATH`, add it once in your shell profile.

The binary uses CoreAudio on macOS and the ALSA runtime library on Linux.
HTTP, TLS, and AAC decoding are built in. The installer prints an optional
hint for installing mpv, which covers HE-AAC streams and native-engine failures.

### Troubleshooting

**`Could not start mpv: No such file or directory`** — mpv is not installed.
mpv is needed for HE-AAC streams (the 128k and 64k fallbacks), when forced
with `RADIOME_ENGINE=mpv`, or after a native-engine failure in `auto` mode.
Install mpv (`brew install mpv` on macOS, `apt install mpv` on Debian/Ubuntu)
to cover those, or pick another station.

**`mpv did not open its IPC socket in 20s`** — only shown by the optional mpv
engine (`RADIOME_ENGINE=mpv`, an HE-AAC-only station, or native failure in `auto`).
mpv started but did not respond in time. On macOS this happens on the very
first launch of a freshly installed mpv, while the system verifies its
libraries. Press Enter to retry; the second start is fast. Running
`mpv --version` once from the terminal after installing has the same effect.
If it keeps happening, check that `/tmp` is writable, since the IPC socket
lives in a temporary directory there.

**`<host> unreachable · Enter to retry`** — the player could not open any of
the station's streams. radiome falls back from the direct streams to HLS and
then retries the whole list two more times, 2 seconds apart, before showing
this; the message names the hosts that failed. The HLS host is a rotating DNS
pool, so a retry can land on an address that works. A VPN or split-tunnel rule
that sends some of these addresses around the tunnel is a common cause: route
`*.hostingradio.ru` and `*.magonet.ru` through the VPN, or check with
`curl -sI https://radiorecord.hostingradio.ru/rr_main96.aacp`. Other errors
are shown as `Stream unavailable: <reason>`.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radiome/master/scripts/uninstall.sh | sh
```

This removes `~/.local/bin/radiome` (or `$RADIOME_PREFIX/bin/radiome` if you installed
with a custom prefix) and keeps your favorites and volume. To delete the settings too,
pass `--purge` when running the script directly, or set `RADIOME_PURGE=1` when piping
it from curl:

```sh
curl -fsSL https://raw.githubusercontent.com/isalikov/radiome/master/scripts/uninstall.sh | RADIOME_PURGE=1 sh
```

Settings live in `~/.config/radiome` by default; the script honours the same
`RADIOME_CONFIG_DIR` and `XDG_CONFIG_HOME` overrides as the app.

## Support

If radiome is useful to you, you can support its development on Ko-fi:

<p align="center">
  <a href="https://ko-fi.com/isalikov"><img src="https://ko-fi.com/img/githubbutton_sm.svg" alt="Support radiome on Ko-fi"></a>
</p>

Bug reports, station suggestions, and small improvements are welcome too.

## Keyboard

| Key | Action |
| --- | --- |
| Tab / Shift+Tab | Next / previous category, wrapping around |
| ↑ / ↓, j / k | Select a station without changing playback |
| Enter | Play selected station |
| F7 / F9 | Play previous / next station in the current category |
| F8 / Space | Play / pause |
| - / = | Player volume, 5% steps, 0–100% |
| f | Toggle favorite |
| i | Show / hide recent tracks |
| d | Toggle diagnostics overlay |
| PgUp / PgDn, Home / End | Scroll stations |
| r | Refresh stations and history |
| s | Stop |
| ? / Esc | Open / close help |
| q / Ctrl+C | Quit |

Previous/next follows the playing station when it is in the current category;
otherwise it starts at the selected station. Unavailable streams are skipped.
Function transport keys also work while help is open.
Left/right arrows have no action. History is read-only. There is no search.

Favorites have a small magenta `+` outside the Favorites category. Inside Favorites,
the category itself identifies them, so the marker is hidden. Station and track
names retain their original spelling from the API; application labels are English.

Terminal F7/F8/F9 and forwarded media-key events are supported in every
engine. On macOS, radiome registers system previous/play-pause/next controls
for both native and mpv playback. The hardware media keys work while radiome
owns the active media session, including when its terminal is unfocused.
The session shows the station name and playback state and is cleared on
stop, playback failure, or exit. Fn+F7/F8/F9 sends ordinary function keys to
the terminal. No global keyboard interception is installed.

## Audio and settings

Native audio uses HTTP (rustls), a pure-Rust AAC-LC decoder (Symphonia),
and a fixed-capacity ring feeding CoreAudio or ALSA. The output callback
resamples up with linear interpolation when the device offers only rates
above the stream's. Slower devices and unsupported stereo configurations
are refused. In `auto`, a device failure starts the optional mpv fallback.

Native pause silences output on the next callback and discards queued audio,
including the resampler's look-ahead. The reader keeps draining the radio;
resume buffers fresh audio. HLS rejoins recent published segments, so its
normal segment delay still applies. The mpv engine uses mpv's pause/cache behavior.

HLS segments are fed completely as ring space becomes available; they are
not truncated to the ring's capacity. The output waits for 0.3 seconds of
audio at startup and after starvation. Its buffering percentage measures
progress toward that threshold. Cache fill, underruns, and audio levels come
from the output callback, so they follow consumption even while HTTP is idle.

Failed URLs fall through to the next supported stream, with three rounds
and a two-second delay between rounds. Each URL change resets the ring,
output sample rate, and telemetry. Stalled HLS playlists and repeated corrupt
segments trigger fallback instead of leaving playback silent indefinitely.
The `d` diagnostics overlay shows cache fill and underruns.

One smooth bar follows the audio before software gain, riding the louder channel. Its scale
adapts to the last four seconds of the station's loudness, leaving headroom
for beats. Attack and release are eased; there is no scrolling, peak marker,
or synthetic animation. Silence, pause and buffering settle the bar, and mono
sources mirror the left channel. Physical stroke thickness is determined by
the terminal font, not an exact pixel size.

Press `d` for a diagnostics overlay: player state and generation, codec, sample
rate, channels, bitrate, the active stream and its fallback round, cache fill
and underruns, worker health, history age, uptime, and a sparkline of recent
levels. The built-in engine measures telemetry in-process; the mpv engine
reports it over its private IPC. While a stream buffers, the status symbol
shows the real cache percentage instead of a bare dot. If the player worker
stops reporting for two seconds while audio should be alive, the footer shows
`Player engine stalled · q quit`.

The UI and application logic use Rust, Ratatui and Crossterm. The built-in
engine decodes AAC in-process; if mpv is installed it is used as a fallback
engine (see `RADIOME_ENGINE` above). macOS system media controls belong to
radiome and remain available across engine changes. Volume is software gain
and does not change system volume.

Favorites and volume are saved atomically in `~/.config/radiome/settings.json`.
Directory precedence: `RADIOME_CONFIG_DIR`, `$XDG_CONFIG_HOME/radiome`,
`~/.config/radiome`. Corrupt settings are reported rather than overwritten.

History and current track metadata refresh every 15 seconds. The API may lag
behind audio, especially after pausing. `RADIOME_BASE_URL` overrides the API
base URL for development. Requests time out after 15 seconds.
Stream priority: `stream_320`, `stream_hls`, `stream_128`, `stream_64`. If a stream
fails to load, the player tries the next distinct one before reporting an error.

## Verify

```sh
make build
make test
make check
make test-audio
```

`make test` runs offline: stream fixtures recorded from the real streams are
decoded, and the engine pipeline is tested against a null output sink using
the same callback as the real device. Tests cover long segments, pause and
resume, stale buffers, station changes, and engine fallback. Live
checks that need the network run with
`cargo test live_ -- --ignored` (direct stream, HLS, and a short burst on the
real audio device). The HLS check observes multiple segment publications
for continuity. `make test-audio` exercises the optional mpv fallback
engine with a sine wave and silent output: audio levels, software volume,
media play/pause bindings, and previous/next delivery over its IPC. It needs
mpv installed and is skipped in every other respect.
