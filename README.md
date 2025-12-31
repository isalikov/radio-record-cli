# 📻 Radio Record CLI

Terminal-based radio player for [Radio Record](https://www.radiorecord.ru/) stations.

![Go Version](https://img.shields.io/badge/Go-1.21+-00ADD8?style=flat&logo=go)
![License](https://img.shields.io/badge/License-MIT-green.svg)
![Platform](https://img.shields.io/badge/Platform-macOS%20%7C%20Linux-lightgrey)

```
📻 Radio Record CLI                                    🔊 ████████░░ 80%
 Все   BASS  BREAKS  CHILL  DRUM  HARD  HOUSE  OLDSCHOOL  POP  RAP/URBAN
──────────────────────────────────────────────────────────────────────────
▸ [1] ♥ Record              Танцевальный мейнстрим
  [2] ♥ Deep                Глубокое house-звучание
       Russian Mix          Русские хиты в танцевальной обработке
  ♪    Chill-Out            Расслабляющий вайб
       Megamix              Mash-up из главных хитов Рекорда
       Remix                Иностранные хиты в танцевальной обработке
──────────────────────────────────────────────────────────────────────────
 4/117 станций
╭────────────────────────────────────────────────────────────────────────╮
│ ▶ JOHN SUMMIT/INEZ — light years (Record Mix)                         │
╰────────────────────────────────────────────────────────────────────────╯
      ? справка │ / поиск │ ←Tab→ жанры │ 0 сброс │ f ♥ │ +/- 🔊 │ Enter ▶
```

## Features

- 🎵 **117 radio stations** — all Radio Record stations
- 🔍 **Vim-style search** — `/` to search, `n`/`N` to navigate matches
- 🎨 **Genre filter** — filter stations by genre with Tab
- ♥ **Favorites** — save your favorite stations, access with `1-9` hotkeys
- 🔊 **Volume control** — adjust volume without leaving the app
- 📺 **Now Playing** — see current track with auto-refresh
- 📐 **Responsive UI** — adapts to terminal size
- 💾 **Persistent config** — favorites and volume saved between sessions

## Installation

### Homebrew (macOS/Linux)

```bash
brew tap isalikov/tap
brew install radio-record-cli
```

### Arch Linux (AUR)

```bash
yay -S radio-record-cli
```

### Debian/Ubuntu

```bash
# Download .deb from releases
wget https://github.com/isalikov/radio-record-cli/releases/latest/download/radio-record-cli_1.0.0_linux_amd64.deb
sudo dpkg -i radio-record-cli_1.0.0_linux_amd64.deb
```

### Fedora/RHEL

```bash
# Download .rpm from releases
wget https://github.com/isalikov/radio-record-cli/releases/latest/download/radio-record-cli_1.0.0_linux_amd64.rpm
sudo rpm -i radio-record-cli_1.0.0_linux_amd64.rpm
```

### Manual

Download the latest binary from [Releases](https://github.com/isalikov/radio-record-cli/releases).

### Build from source

```bash
git clone https://github.com/isalikov/radio-record-cli.git
cd radio-record-cli
make build
./radio-record
```

## Requirements

- **mpv** — used for audio playback

```bash
# macOS
brew install mpv

# Ubuntu/Debian
sudo apt install mpv

# Arch Linux
sudo pacman -S mpv

# Fedora
sudo dnf install mpv
```

## Usage

```bash
radio-record
```

### Keybindings

| Key | Action |
|-----|--------|
| `j` / `↓` | Move down |
| `k` / `↑` | Move up |
| `g` | Go to top |
| `G` | Go to bottom |
| `Enter` / `Space` | Play station |
| `s` | Stop playback |
| `+` / `=` | Volume up |
| `-` / `_` | Volume down |
| `/` | Start search |
| `n` | Next search match |
| `N` | Previous search match |
| `Esc` | Clear search |
| `Tab` | Next genre |
| `Shift+Tab` | Previous genre |
| `0` | Reset all filters |
| `f` | Toggle favorite |
| `F` | Show only favorites |
| `1-9` | Play favorite #1-9 |
| `?` | Show help |
| `q` | Quit |

## Configuration

Config is stored at:
- macOS: `~/Library/Application Support/radio-record-cli/config.json`
- Linux: `~/.config/radio-record-cli/config.json`

```json
{
  "favorites": [15016, 15018, 15020],
  "volume": 80
}
```

## API

This player uses the public Radio Record API:
- `GET /api/stations/` — list of all stations
- `GET /api/station/history/?id={id}` — current track and history

## License

MIT License. See [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request.

1. Fork the repository
2. Create your feature branch (`git checkout -b feature/amazing-feature`)
3. Commit your changes (`git commit -m 'Add some amazing feature'`)
4. Push to the branch (`git push origin feature/amazing-feature`)
5. Open a Pull Request

## Acknowledgments

- [Radio Record](https://www.radiorecord.ru/) for the awesome radio stations
- [Bubble Tea](https://github.com/charmbracelet/bubbletea) for the TUI framework
- [Lip Gloss](https://github.com/charmbracelet/lipgloss) for terminal styling
- [mpv](https://mpv.io/) for audio playback
