use std::process::Command;

use crate::error::{Error, Result};
use crate::json::{self, JsonValue};

const BASE_URL: &str = "https://www.radiorecord.ru/api";
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";

#[derive(Debug, Clone, Default)]
pub struct Client {
    base_url: String,
}

impl Client {
    pub fn new() -> Self {
        let base_url = std::env::var("RADIOME_BASE_URL").unwrap_or_else(|_| BASE_URL.to_string());
        Self::with_base_url(base_url)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        Self {
            base_url: base_url.into(),
        }
    }

    pub fn get_catalog(&self) -> Result<Catalog> {
        let value = self.fetch_json("/stations/")?;
        decode_catalog(&value)
    }

    pub fn get_history(&self, station_id: i64, limit: usize) -> Result<Vec<Track>> {
        let value = self.fetch_json(&format!("/station/history/?id={station_id}"))?;
        let mut history = decode_history(&value)?;
        if limit > 0 && history.len() > limit {
            history.truncate(limit);
        }
        Ok(history)
    }

    fn fetch_json(&self, path: &str) -> Result<JsonValue> {
        let url = format!("{}{}", self.base_url, path);
        let output = Command::new("curl")
            .args([
                "-fsSL",
                "--compressed",
                "--connect-timeout",
                "5",
                "--max-time",
                "15",
                "-A",
                USER_AGENT,
                &url,
            ])
            .output()
            .map_err(|err| Error::new(format!("failed to run curl: {err}")))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(Error::new(format!(
                "curl failed for {url}: {}",
                stderr.trim()
            )));
        }

        let body = String::from_utf8(output.stdout)
            .map_err(|err| Error::new(format!("invalid UTF-8 from curl: {err}")))?;
        json::parse(&body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub stations: Vec<Station>,
    pub genres: Vec<Genre>,
    pub tags: Vec<Genre>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Station {
    pub id: i64,
    pub prefix: String,
    pub title: String,
    pub tooltip: String,
    pub stream_64: String,
    pub stream_128: String,
    pub stream_320: String,
    pub stream_hls: String,
    pub icon_fill: String,
    pub genres: Vec<Genre>,
}

impl Station {
    pub fn stream_url(&self) -> Option<&str> {
        self.stream_urls().into_iter().next()
    }

    // Every distinct stream in priority order; the player falls back down the list when
    // a host is unreachable (stream_320/128/64 share one host, HLS lives on another).
    pub fn stream_urls(&self) -> Vec<&str> {
        let mut urls = Vec::new();
        for candidate in [
            self.stream_320.as_str(),
            self.stream_hls.as_str(),
            self.stream_128.as_str(),
            self.stream_64.as_str(),
        ] {
            if !candidate.is_empty() && !urls.contains(&candidate) {
                urls.push(candidate);
            }
        }
        urls
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub id: i64,
    pub artist: String,
    pub song: String,
    pub image100: String,
    pub image200: String,
    pub time_formatted: String,
}

fn decode_catalog(value: &JsonValue) -> Result<Catalog> {
    let result = value
        .object_get("result")
        .ok_or_else(|| Error::new("missing result object in stations response"))?;

    Ok(Catalog {
        stations: decode_station_list(
            result
                .object_get("stations")
                .ok_or_else(|| Error::new("missing stations array"))?,
        )?,
        genres: decode_genre_list(
            result
                .object_get("genre")
                .ok_or_else(|| Error::new("missing genre array"))?,
        )?,
        tags: decode_genre_list(
            result
                .object_get("tags")
                .ok_or_else(|| Error::new("missing tags array"))?,
        )?,
    })
}

fn decode_history(value: &JsonValue) -> Result<Vec<Track>> {
    let result = value
        .object_get("result")
        .ok_or_else(|| Error::new("missing result object in history response"))?;
    decode_track_list(
        result
            .object_get("history")
            .ok_or_else(|| Error::new("missing history array"))?,
    )
}

fn decode_station_list(value: &JsonValue) -> Result<Vec<Station>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::new("expected stations to be an array"))?;
    items.iter().map(decode_station).collect()
}

fn decode_genre_list(value: &JsonValue) -> Result<Vec<Genre>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::new("expected genre list to be an array"))?;
    items.iter().map(decode_genre).collect()
}

fn decode_track_list(value: &JsonValue) -> Result<Vec<Track>> {
    let items = value
        .as_array()
        .ok_or_else(|| Error::new("expected history to be an array"))?;
    items.iter().map(decode_track).collect()
}

fn decode_station(value: &JsonValue) -> Result<Station> {
    Ok(Station {
        id: required_i64(value, "id")?,
        prefix: required_string(value, "prefix")?,
        title: required_string(value, "title")?,
        tooltip: required_string(value, "tooltip")?,
        stream_64: optional_string(value, "stream_64"),
        stream_128: optional_string(value, "stream_128"),
        stream_320: optional_string(value, "stream_320"),
        stream_hls: optional_string(value, "stream_hls"),
        icon_fill: optional_string(value, "icon_fill_colored"),
        genres: value
            .object_get("genre")
            .and_then(JsonValue::as_array)
            .map(|items| items.iter().map(decode_genre).collect::<Result<Vec<_>>>())
            .transpose()?
            .unwrap_or_default(),
    })
}

fn decode_genre(value: &JsonValue) -> Result<Genre> {
    Ok(Genre {
        id: required_i64(value, "id")?,
        name: required_string(value, "name")?,
    })
}

fn decode_track(value: &JsonValue) -> Result<Track> {
    Ok(Track {
        id: required_i64(value, "id")?,
        artist: optional_string(value, "artist"),
        song: optional_string(value, "song"),
        image100: optional_string(value, "image100"),
        image200: optional_string(value, "image200"),
        time_formatted: optional_string(value, "time_formatted"),
    })
}

fn required_string(value: &JsonValue, key: &str) -> Result<String> {
    value
        .object_get(key)
        .and_then(JsonValue::as_str)
        .map(ToString::to_string)
        .ok_or_else(|| Error::new(format!("missing string field: {key}")))
}

fn optional_string(value: &JsonValue, key: &str) -> String {
    value
        .object_get(key)
        .and_then(JsonValue::as_str)
        .unwrap_or("")
        .to_string()
}

fn required_i64(value: &JsonValue, key: &str) -> Result<i64> {
    value
        .object_get(key)
        .and_then(JsonValue::as_i64)
        .ok_or_else(|| Error::new(format!("missing integer field: {key}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_catalog_and_stream_url() {
        let sample = r##"
        {
          "result": {
            "stations": [
              {
                "id": 1,
                "prefix": "test",
                "title": "Test Station",
                "tooltip": "Test description",
                "stream_320": "https://example.com/stream.mp3",
                "stream_hls": "",
                "stream_128": "",
                "stream_64": "",
                "icon_fill_colored": "#ffffff",
                "genre": [{ "id": 2, "name": "HOUSE" }]
              }
            ],
            "genre": [{ "id": 2, "name": "HOUSE" }],
            "tags": []
          }
        }
        "##;

        let value = json::parse(sample).unwrap();
        let catalog = decode_catalog(&value).unwrap();
        assert_eq!(catalog.stations.len(), 1);
        assert_eq!(catalog.genres.len(), 1);
        assert_eq!(catalog.tags.len(), 0);
        assert_eq!(
            catalog.stations[0].stream_url(),
            Some("https://example.com/stream.mp3")
        );
        let mut station = catalog.stations[0].clone();
        station.stream_hls = "https://example.com/playlist.m3u8".into();
        station.stream_128 = station.stream_320.clone();
        station.stream_64 = "https://example.com/stream64.aacp".into();
        assert_eq!(
            station.stream_urls(),
            [
                "https://example.com/stream.mp3",
                "https://example.com/playlist.m3u8",
                "https://example.com/stream64.aacp",
            ]
        );
    }

    #[test]
    fn parses_history() {
        let sample = r#"
        {
          "result": {
            "history": [
              {
                "id": 123,
                "artist": "Test Artist",
                "song": "Test Song",
                "image100": "",
                "image200": "",
                "time_formatted": "12:34:56"
              }
            ]
          }
        }
        "#;

        let value = json::parse(sample).unwrap();
        let history = decode_history(&value).unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].artist, "Test Artist");
        assert_eq!(history[0].song, "Test Song");
    }
}
