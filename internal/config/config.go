package config

import (
	"encoding/json"
	"os"
	"path/filepath"
)

type Config struct {
	Favorites []int `json:"favorites"` // Station IDs
	Volume    int   `json:"volume"`
	path      string
}

func Load() (*Config, error) {
	configDir, err := os.UserConfigDir()
	if err != nil {
		configDir = os.Getenv("HOME")
	}

	configPath := filepath.Join(configDir, "radio-record-cli", "config.json")

	cfg := &Config{
		Favorites: []int{},
		Volume:    80,
		path:      configPath,
	}

	data, err := os.ReadFile(configPath)
	if err != nil {
		return cfg, nil // Return default config if file doesn't exist
	}

	json.Unmarshal(data, cfg)
	cfg.path = configPath
	return cfg, nil
}

func (c *Config) Save() error {
	dir := filepath.Dir(c.path)
	if err := os.MkdirAll(dir, 0755); err != nil {
		return err
	}

	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return err
	}

	return os.WriteFile(c.path, data, 0644)
}

func (c *Config) IsFavorite(stationID int) bool {
	for _, id := range c.Favorites {
		if id == stationID {
			return true
		}
	}
	return false
}

func (c *Config) ToggleFavorite(stationID int) {
	if c.IsFavorite(stationID) {
		// Remove
		newFavs := []int{}
		for _, id := range c.Favorites {
			if id != stationID {
				newFavs = append(newFavs, id)
			}
		}
		c.Favorites = newFavs
	} else {
		// Add
		c.Favorites = append(c.Favorites, stationID)
	}
	c.Save()
}
