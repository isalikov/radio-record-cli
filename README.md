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

Requires Rust/Cargo, curl with compression support, and mpv with FFmpeg's astats filter.
Supports macOS and Linux.

The Rust toolchain is pinned in `mise.toml`. With [mise](https://mise.jdx.dev) installed,
`mise install` in this directory fetches the right `cargo`, `rustfmt`, and `clippy`.
Any other Rust installation (rustup, distro package) works as well. mpv is a system
package; on macOS `brew install mpv`, on Debian/Ubuntu `apt install mpv`.

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

The installer does not install mpv or curl. It checks for both at the end and prints
the install command for your platform if either is missing.

### Troubleshooting

**`Could not start mpv: No such file or directory`** — mpv is not installed or not
in `PATH`. Install it (`brew install mpv` on macOS, `apt install mpv` on Debian/Ubuntu)
and make sure `mpv --version` works in the same terminal you run radiome from.

**`mpv did not open its IPC socket in 20s`** — mpv started but did not respond in
time. On macOS this happens on the very first launch of a freshly installed mpv,
while the system verifies its libraries. Press Enter to retry; the second start is
fast. Running `mpv --version` once from the terminal after installing has the same
effect. If it keeps happening, check that `/tmp` is writable, since the IPC socket
lives in a temporary directory there.

**`<host> unreachable · Enter to retry`** — mpv could not open any of the station's
streams. radiome falls back from the direct streams to HLS and then retries the whole
list two more times, 2 seconds apart, before showing this; the message names the
hosts that failed. The HLS host is a rotating DNS pool, so a retry can land on an
address that works. A VPN or split-tunnel rule that sends some of these addresses
around the tunnel is a common cause: route `*.hostingradio.ru` and `*.magonet.ru`
through the VPN, or check with
`curl -sI https://radiorecord.hostingradio.ru/rr_main96.aacp`. Other mpv errors are
shown as `Stream unavailable: <reason>`.

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
`RADIOME_CONFIG_DIR` and `XDG_CONFIG_HOME` overrides as the app. mpv and curl are left
installed since other software may use them.

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

Terminal F7/F8/F9 and forwarded media-key events are supported. On macOS, once
playback has started, mpv's system media controls route previous/next to Rust and
play/pause to the audio engine. This uses mpv's
[RemoteCommandCenter integration](https://github.com/mpv-player/mpv/blob/v0.41.0/osdep/mac/remote_command_center.swift).
If macOS handles your top-row keys as system controls, Fn+F7/F8/F9 sends ordinary
function keys to the terminal. No global keyboard interception is installed.

## Audio and settings

Two thin lines follow the decoded left and right audio levels at 20 Hz, with a
bold tick marking the slowly falling peak hold. Their scale adapts to the last
four seconds of the station's loudness, leaving headroom for beats. Attack and
release are smoothed; there is no scrolling or synthetic animation. Silence,
pause and buffering settle both lines, and mono sources mirror the left channel.
Physical stroke thickness is determined by the terminal font, not an exact pixel
size.

Press `d` for a diagnostics overlay: player state and generation, codec, sample
rate, channels, bitrate, the active stream and its fallback round, cache fill,
worker health, history age, uptime, and a sparkline of recent levels. Telemetry
arrives over the existing mpv IPC on the worker thread; volatile properties are
polled at a low rate and only while they are meaningful. While a stream buffers,
the status symbol shows the real cache percentage instead of a bare dot. If the
player worker stops reporting for two seconds while audio should be alive, the
footer shows `Player engine stalled · q quit`.

The UI and application logic use Rust, Ratatui and Crossterm. Audio uses an
external mpv process with private IPC; curl fetches API data on worker threads.
Volume is mpv's software gain and does not change system volume.

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

The audio test uses real mpv with a sine wave and silent output. It checks
audio levels, software volume, media play/pause bindings, and previous/next
delivery over IPC. Physical hardware key routing still depends on macOS and
which media session is active.
