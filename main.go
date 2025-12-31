package main

import (
	"fmt"
	"os"
	"os/exec"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/isalikov/radio-record-cli/internal/api"
	"github.com/isalikov/radio-record-cli/internal/config"
	"github.com/isalikov/radio-record-cli/internal/player"
	"github.com/isalikov/radio-record-cli/internal/ui"
)

var (
	version = "dev"
	commit  = "none"
	date    = "unknown"
)

func main() {
	// Version flag
	if len(os.Args) > 1 && (os.Args[1] == "--version" || os.Args[1] == "-v") {
		fmt.Printf("radio-record-cli %s\n", version)
		fmt.Printf("  commit: %s\n", commit)
		fmt.Printf("  built:  %s\n", date)
		os.Exit(0)
	}

	// Check if mpv is installed
	if _, err := exec.LookPath("mpv"); err != nil {
		fmt.Println("Ошибка: mpv не найден. Установите mpv:")
		fmt.Println("  macOS:  brew install mpv")
		fmt.Println("  Linux:  sudo apt install mpv")
		fmt.Println("  Windows: winget install mpv")
		os.Exit(1)
	}

	// Load config
	cfg, err := config.Load()
	if err != nil {
		fmt.Printf("Ошибка загрузки конфига: %v\n", err)
		os.Exit(1)
	}

	client := api.NewClient()
	p := player.New()

	// Set volume from config
	p.SetVolume(cfg.Volume)

	model := ui.NewModel(client, p, cfg)

	program := tea.NewProgram(model, tea.WithAltScreen())

	if _, err := program.Run(); err != nil {
		fmt.Printf("Ошибка: %v\n", err)
		os.Exit(1)
	}

	// Save volume to config on exit
	cfg.Volume = p.Volume()
	cfg.Save()
}
