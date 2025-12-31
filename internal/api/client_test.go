package api

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"testing"
)

func TestGetStations(t *testing.T) {
	// Mock server
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/stations/" {
			t.Errorf("Expected /stations/, got %s", r.URL.Path)
		}

		response := stationsResponse{
			Result: struct {
				Stations []Station `json:"stations"`
				Genres   []Genre   `json:"genre"`
			}{
				Stations: []Station{
					{
						ID:        1,
						Prefix:    "test",
						Title:     "Test Station",
						Tooltip:   "Test description",
						Stream320: "https://example.com/stream.mp3",
					},
					{
						ID:        2,
						Prefix:    "test2",
						Title:     "Test Station 2",
						Tooltip:   "Another test",
						Stream320: "https://example.com/stream2.mp3",
					},
				},
				Genres: []Genre{
					{ID: 1, Name: "HOUSE"},
					{ID: 2, Name: "TECHNO"},
				},
			},
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	// Create client with mock server URL
	client := &Client{
		http:    server.Client(),
		baseURL: server.URL,
	}

	stations, err := client.GetStations()
	if err != nil {
		t.Fatalf("GetStations failed: %v", err)
	}

	if len(stations) != 2 {
		t.Errorf("Expected 2 stations, got %d", len(stations))
	}

	if stations[0].Title != "Test Station" {
		t.Errorf("Expected 'Test Station', got %s", stations[0].Title)
	}

	if stations[0].ID != 1 {
		t.Errorf("Expected ID 1, got %d", stations[0].ID)
	}
}

func TestGetNowPlaying(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		response := historyResponse{
			Result: struct {
				History []Track `json:"history"`
			}{
				History: []Track{
					{
						ID:            123,
						Artist:        "Test Artist",
						Song:          "Test Song",
						TimeFormatted: "12:34:56",
					},
				},
			},
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	client := &Client{
		http:    server.Client(),
		baseURL: server.URL,
	}

	track, err := client.GetNowPlaying(1)
	if err != nil {
		t.Fatalf("GetNowPlaying failed: %v", err)
	}

	if track == nil {
		t.Fatal("Expected track, got nil")
	}

	if track.Artist != "Test Artist" {
		t.Errorf("Expected 'Test Artist', got %s", track.Artist)
	}

	if track.Song != "Test Song" {
		t.Errorf("Expected 'Test Song', got %s", track.Song)
	}
}

func TestGetNowPlayingEmpty(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		response := historyResponse{
			Result: struct {
				History []Track `json:"history"`
			}{
				History: []Track{},
			},
		}

		w.Header().Set("Content-Type", "application/json")
		json.NewEncoder(w).Encode(response)
	}))
	defer server.Close()

	client := &Client{
		http:    server.Client(),
		baseURL: server.URL,
	}

	track, err := client.GetNowPlaying(1)
	if err != nil {
		t.Fatalf("GetNowPlaying failed: %v", err)
	}

	if track != nil {
		t.Errorf("Expected nil track for empty history, got %v", track)
	}
}
