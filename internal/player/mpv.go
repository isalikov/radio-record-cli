package player

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"os/exec"
	"path/filepath"
	"sync"
	"time"
)

type Player struct {
	cmd        *exec.Cmd
	streamURL  string
	playing    bool
	volume     int
	socketPath string
	mu         sync.Mutex
}

func New() *Player {
	socketPath := filepath.Join(os.TempDir(), fmt.Sprintf("radiorecord-mpv-%d.sock", os.Getpid()))
	return &Player{
		volume:     80,
		socketPath: socketPath,
	}
}

// Play starts playing the given stream URL
func (p *Player) Play(url string) error {
	p.mu.Lock()
	defer p.mu.Unlock()

	// Stop current playback if any
	if p.cmd != nil && p.cmd.Process != nil {
		p.cmd.Process.Kill()
		p.cmd.Wait()
	}

	// Remove old socket
	os.Remove(p.socketPath)

	p.streamURL = url
	p.cmd = exec.Command("mpv",
		"--no-video",
		"--quiet",
		"--no-terminal",
		fmt.Sprintf("--volume=%d", p.volume),
		fmt.Sprintf("--input-ipc-server=%s", p.socketPath),
		url,
	)

	if err := p.cmd.Start(); err != nil {
		return err
	}

	p.playing = true
	return nil
}

// Stop stops the current playback
func (p *Player) Stop() {
	p.mu.Lock()
	defer p.mu.Unlock()

	if p.cmd != nil && p.cmd.Process != nil {
		p.cmd.Process.Kill()
		p.cmd.Wait()
		p.cmd = nil
	}
	os.Remove(p.socketPath)
	p.playing = false
}

// sendCommand sends a command to mpv via IPC socket
func (p *Player) sendCommand(command []interface{}) error {
	conn, err := net.DialTimeout("unix", p.socketPath, 100*time.Millisecond)
	if err != nil {
		return err
	}
	defer conn.Close()

	msg := map[string]interface{}{
		"command": command,
	}
	data, _ := json.Marshal(msg)
	data = append(data, '\n')
	_, err = conn.Write(data)
	return err
}

// SetVolume sets the playback volume (0-100)
func (p *Player) SetVolume(vol int) {
	p.mu.Lock()
	defer p.mu.Unlock()

	if vol < 0 {
		vol = 0
	}
	if vol > 100 {
		vol = 100
	}
	p.volume = vol

	if p.playing {
		p.sendCommand([]interface{}{"set_property", "volume", vol})
	}
}

// Volume returns current volume level
func (p *Player) Volume() int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.volume
}

// VolumeUp increases volume by 5
func (p *Player) VolumeUp() {
	p.SetVolume(p.Volume() + 5)
}

// VolumeDown decreases volume by 5
func (p *Player) VolumeDown() {
	p.SetVolume(p.Volume() - 5)
}

// IsPlaying returns true if player is currently playing
func (p *Player) IsPlaying() bool {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.playing
}

// CurrentURL returns the currently playing stream URL
func (p *Player) CurrentURL() string {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.streamURL
}
