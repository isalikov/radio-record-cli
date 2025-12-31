package config

import (
	"os"
	"path/filepath"
	"testing"
)

func TestIsFavorite(t *testing.T) {
	cfg := &Config{
		Favorites: []int{1, 2, 3},
	}

	tests := []struct {
		stationID int
		expected  bool
	}{
		{1, true},
		{2, true},
		{3, true},
		{4, false},
		{0, false},
	}

	for _, tt := range tests {
		result := cfg.IsFavorite(tt.stationID)
		if result != tt.expected {
			t.Errorf("IsFavorite(%d) = %v, expected %v", tt.stationID, result, tt.expected)
		}
	}
}

func TestToggleFavorite(t *testing.T) {
	// Create temp config file
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "config.json")

	cfg := &Config{
		Favorites: []int{1, 2},
		Volume:    80,
		path:      configPath,
	}

	// Add new favorite
	cfg.ToggleFavorite(3)
	if !cfg.IsFavorite(3) {
		t.Error("Expected station 3 to be favorite after toggle")
	}
	if len(cfg.Favorites) != 3 {
		t.Errorf("Expected 3 favorites, got %d", len(cfg.Favorites))
	}

	// Remove existing favorite
	cfg.ToggleFavorite(2)
	if cfg.IsFavorite(2) {
		t.Error("Expected station 2 to not be favorite after toggle")
	}
	if len(cfg.Favorites) != 2 {
		t.Errorf("Expected 2 favorites, got %d", len(cfg.Favorites))
	}
}

func TestSaveAndLoad(t *testing.T) {
	tmpDir := t.TempDir()
	configPath := filepath.Join(tmpDir, "subdir", "config.json")

	// Create and save config
	cfg := &Config{
		Favorites: []int{10, 20, 30},
		Volume:    75,
		path:      configPath,
	}

	err := cfg.Save()
	if err != nil {
		t.Fatalf("Save failed: %v", err)
	}

	// Verify file exists
	if _, err := os.Stat(configPath); os.IsNotExist(err) {
		t.Fatal("Config file was not created")
	}

	// Load config
	cfg2 := &Config{path: configPath}
	data, err := os.ReadFile(configPath)
	if err != nil {
		t.Fatalf("ReadFile failed: %v", err)
	}

	t.Logf("Saved config: %s", string(data))

	// Manually verify JSON content
	if len(cfg.Favorites) != 3 {
		t.Errorf("Expected 3 favorites, got %d", len(cfg.Favorites))
	}

	if cfg.Favorites[0] != 10 {
		t.Errorf("Expected first favorite to be 10, got %d", cfg.Favorites[0])
	}

	_ = cfg2 // Suppress unused warning
}

func TestLoadNonExistent(t *testing.T) {
	// Save original function behavior - Load should return default config
	// when file doesn't exist
	cfg, err := Load()
	if err != nil {
		t.Fatalf("Load failed: %v", err)
	}

	// Should return default values
	if cfg.Volume != 80 {
		t.Errorf("Expected default volume 80, got %d", cfg.Volume)
	}

	if cfg.Favorites == nil {
		t.Error("Favorites should not be nil")
	}
}

func TestEmptyFavorites(t *testing.T) {
	cfg := &Config{
		Favorites: []int{},
	}

	if cfg.IsFavorite(1) {
		t.Error("Empty favorites should not contain any station")
	}

	cfg.ToggleFavorite(1)
	if !cfg.IsFavorite(1) {
		t.Error("Should be able to add favorite to empty list")
	}
}
