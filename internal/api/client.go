package api

import (
	"encoding/json"
	"fmt"
	"net/http"
	"time"
)

const (
	baseURL    = "https://www.radiorecord.ru/api"
	userAgent  = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36"
)

type Client struct {
	http    *http.Client
	baseURL string
}

func NewClient() *Client {
	return &Client{
		http: &http.Client{
			Timeout: 10 * time.Second,
		},
		baseURL: baseURL,
	}
}

// Station represents a radio station
type Station struct {
	ID         int      `json:"id"`
	Prefix     string   `json:"prefix"`
	Title      string   `json:"title"`
	Tooltip    string   `json:"tooltip"`
	Stream64   string   `json:"stream_64"`
	Stream128  string   `json:"stream_128"`
	Stream320  string   `json:"stream_320"`
	StreamHLS  string   `json:"stream_hls"`
	IconFill   string   `json:"icon_fill_colored"`
	Genres     []Genre  `json:"genre"`
}

type Genre struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
}

type Track struct {
	ID            int    `json:"id"`
	Artist        string `json:"artist"`
	Song          string `json:"song"`
	Image100      string `json:"image100"`
	Image200      string `json:"image200"`
	TimeFormatted string `json:"time_formatted"`
}

type stationsResponse struct {
	Result struct {
		Stations []Station `json:"stations"`
		Genres   []Genre   `json:"genre"`
	} `json:"result"`
}

type historyResponse struct {
	Result struct {
		History []Track `json:"history"`
	} `json:"result"`
}

// GetStations fetches all available radio stations
func (c *Client) GetStations() ([]Station, error) {
	req, err := http.NewRequest("GET", c.baseURL+"/stations/", nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", userAgent)

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var result stationsResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}

	return result.Result.Stations, nil
}

// GetNowPlaying fetches current track for a station
func (c *Client) GetNowPlaying(stationID int) (*Track, error) {
	url := fmt.Sprintf("%s/station/history/?id=%d", c.baseURL, stationID)

	req, err := http.NewRequest("GET", url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("User-Agent", userAgent)

	resp, err := c.http.Do(req)
	if err != nil {
		return nil, err
	}
	defer resp.Body.Close()

	var result historyResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}

	if len(result.Result.History) == 0 {
		return nil, nil
	}

	return &result.Result.History[0], nil
}
