use std::collections::BTreeSet;
use std::io::Read;
use std::time::Duration;

use crate::error::{Error, Result};
use crate::json::{self, JsonValue};

const BASE_URL: &str = "https://www.radiorecord.ru/api";
pub(crate) const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone)]
pub struct Client {
    base_url: String,
    http: ureq::Agent,
}

impl Client {
    pub fn new() -> Self {
        let base_url =
            std::env::var("RADIO_RECORD_BASE_URL").unwrap_or_else(|_| BASE_URL.to_string());
        Self::with_base_url(base_url)
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Self {
        let http: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(USER_AGENT)
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .build()
            .into();
        Self {
            base_url: base_url.into(),
            http,
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

    pub fn login(&self, email: &str, password: &str) -> Result<Login> {
        let (content_type, body) = multipart(&[
            ("type", "email"),
            ("email", email),
            ("password", password),
            ("USER_LOGIN", email),
            ("USER_PASSWORD", password),
        ]);
        let response = self
            .http
            .post(format!("{}/auth", self.base_url))
            .header("Content-Type", content_type)
            .send(body)
            .map_err(account_request_error)?;
        let value = account_json(response)?;
        let device_code = value
            .object_get("device_code")
            .and_then(JsonValue::as_str)
            .filter(|code| !code.is_empty())
            .ok_or_else(|| Error::new("Sign-in failed · check email and password"))?
            .to_owned();
        validate_device_code(&device_code)?;
        let profile = self.get_profile(&device_code)?;
        Ok(Login {
            device_code,
            profile,
        })
    }

    pub fn get_profile(&self, device_code: &str) -> Result<Profile> {
        let value = self.account_get("/profile", device_code)?;
        decode_profile(&value)
    }

    /// GET without a cursor returns a full station set. We deliberately do not
    /// persist date_sync: it is a 64-bit cursor, not a Unix timestamp.
    pub fn get_favorites(&self, device_code: &str) -> Result<BTreeSet<i64>> {
        let value = self.account_get("/favorites/sync/", device_code)?;
        decode_favorites(&value)
    }

    pub fn set_favorite(&self, device_code: &str, id: i64, add: bool) -> Result<()> {
        validate_device_code(device_code)?;
        let id = id.to_string();
        let (content_type, body) = multipart(&[
            ("id", &id),
            ("ID", &id),
            ("_method", if add { "undefined" } else { "DELETE" }),
        ]);
        let url = format!("{}/favorites/station/?id={id}", self.base_url);
        let response = if add {
            self.http
                .post(url)
                .header("x-device-code", device_code)
                .header("Content-Type", content_type)
                .send(body)
        } else {
            self.http
                .delete(url)
                .force_send_body()
                .header("x-device-code", device_code)
                .header("Content-Type", content_type)
                .send(body)
        }
        .map_err(account_request_error)?;
        let value = account_json(response)?;
        if value
            .object_get("result")
            .and_then(|v| v.object_get("status"))
            .and_then(JsonValue::as_str)
            != Some("ok")
        {
            return Err(Error::new("Favorite update was not accepted · s retry"));
        }
        Ok(())
    }

    fn account_get(&self, path: &str, device_code: &str) -> Result<JsonValue> {
        validate_device_code(device_code)?;
        let response = self
            .http
            .get(format!("{}{path}", self.base_url))
            .header("x-device-code", device_code)
            .header("Accept", "application/json")
            .call()
            .map_err(account_request_error)?;
        account_json(response)
    }

    fn fetch_json(&self, path: &str) -> Result<JsonValue> {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .get(&url)
            .call()
            .map_err(|err| Error::new(format!("request failed for {url}: {err}")))?;
        let mut body = String::new();
        response
            .into_body()
            .as_reader()
            .read_to_string(&mut body)
            .map_err(|err| Error::new(format!("failed to read response from {url}: {err}")))?;
        json::parse(&body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub id: i64,
    pub email: String,
    pub name: String,
    pub premium: bool,
}

// Never derive Debug: this value contains the account's bearer credential.
pub struct Login {
    pub device_code: String,
    pub profile: Profile,
}

fn validate_device_code(code: &str) -> Result<()> {
    if code.is_empty()
        || !code
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-=+/".contains(&b))
    {
        return Err(Error::new("Invalid account session · sign in again"));
    }
    Ok(())
}

fn account_request_error(error: ureq::Error) -> Error {
    // Do not format the request or response: either could contain credentials.
    match error {
        ureq::Error::StatusCode(401 | 403) => {
            Error::new("Session expired or sign-in denied · sign in again")
        }
        ureq::Error::StatusCode(400) => Error::new("Sign-in failed · check email and password"),
        ureq::Error::StatusCode(code) => {
            Error::new(format!("Account request failed (HTTP {code}) · s retry"))
        }
        _ => Error::new("Account unavailable · check connection and s retry"),
    }
}

fn account_json(response: ureq::http::Response<ureq::Body>) -> Result<JsonValue> {
    // Bound responses from the account API and keep raw server errors out of UI.
    let mut body = String::new();
    response
        .into_body()
        .as_reader()
        .take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut body)
        .map_err(|_| Error::new("Could not read account response · s retry"))?;
    if body.len() > 2 * 1024 * 1024 {
        return Err(Error::new("Account response is too large"));
    }
    json::parse(&body).map_err(|_| Error::new("Invalid account response · s retry"))
}

fn multipart(fields: &[(&str, &str)]) -> (String, String) {
    // Pick a boundary absent from all values; passwords can contain punctuation.
    let mut boundary = "radio-record-form".to_owned();
    while fields.iter().any(|(_, value)| value.contains(&boundary)) {
        boundary.push('x');
    }
    let mut body = String::new();
    for (name, value) in fields {
        body.push_str(&format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{name}\"\r\n\r\n{value}\r\n"
        ));
    }
    body.push_str(&format!("--{boundary}--\r\n"));
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn decode_profile(value: &JsonValue) -> Result<Profile> {
    let user = value
        .object_get("result")
        .ok_or_else(|| Error::new("Profile unavailable · sign in again"))?;
    let id = user
        .object_get("id")
        .and_then(JsonValue::as_i64)
        .filter(|id| *id > 0)
        .ok_or_else(|| Error::new("Session expired · sign in again"))?;
    let text = |key| {
        user.object_get(key)
            .and_then(JsonValue::as_str)
            .unwrap_or_default()
    };
    Ok(Profile {
        id,
        email: text("email").to_owned(),
        name: format!("{} {}", text("firstname"), text("lastname"))
            .trim()
            .to_owned(),
        premium: user.object_get("is_premium") == Some(&JsonValue::Bool(true)),
    })
}

fn decode_favorites(value: &JsonValue) -> Result<BTreeSet<i64>> {
    let stations = value
        .object_get("result")
        .and_then(|v| v.object_get("stations"))
        .ok_or_else(|| Error::new("Favorites unavailable · s retry"))?;
    let ids = |key| -> Result<BTreeSet<i64>> {
        let values = stations
            .object_get(key)
            .and_then(JsonValue::as_array)
            .ok_or_else(|| Error::new("Invalid favorites response · s retry"))?;
        values
            .iter()
            .map(|v| {
                v.as_i64()
                    .filter(|id| *id > 0)
                    .ok_or_else(|| Error::new("Invalid favorite station ID"))
            })
            .collect()
    };
    let mut result = ids("add")?;
    for id in ids("remove")? {
        result.remove(&id);
    }
    Ok(result)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub stations: Vec<Station>,
    pub genres: Vec<Genre>,
    pub tags: Vec<Genre>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// `stream_320`: direct ADTS AAC-LC, ~96 kbps (the field name is historic).
    Main,
    /// `stream_hls`: HLS playlist; the AAC-LC variant is picked when played natively.
    Hls,
    /// `stream_128`: direct HE-AAC (SBR).
    High,
    /// `stream_64`: direct HE-AACv2 (SBR + parametric stereo).
    Low,
}

/// One playable stream with the API field it came from, so each engine can
/// filter the fallback list by what it can actually decode.
#[derive(Debug, Clone)]
pub struct Stream {
    pub kind: StreamKind,
    pub url: String,
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
        [
            &self.stream_320,
            &self.stream_hls,
            &self.stream_128,
            &self.stream_64,
        ]
        .into_iter()
        .find(|url| !url.is_empty())
        .map(|url| url.as_str())
    }

    // Every stream in priority order; the player falls back down the list when
    // a host is unreachable (stream_320/128/64 share one host, HLS lives on
    // another). Duplicate URLs are skipped: retrying the same one is pointless.
    pub fn streams(&self) -> Vec<Stream> {
        let mut collected: Vec<Stream> = Vec::new();
        for (kind, url) in [
            (StreamKind::Main, &self.stream_320),
            (StreamKind::Hls, &self.stream_hls),
            (StreamKind::High, &self.stream_128),
            (StreamKind::Low, &self.stream_64),
        ] {
            if url.is_empty() || collected.iter().any(|stream| stream.url == *url) {
                continue;
            }
            collected.push(Stream {
                kind,
                url: url.clone(),
            });
        }
        collected
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
pub(crate) mod tests {
    use std::io::Write;
    use std::{net::TcpListener, thread, time::Instant};

    pub(crate) const PROFILE: &str = r#"{"result":{"id":42,"email":"test@example.com","firstname":"Test","lastname":"User","is_premium":true}}"#;

    pub(crate) fn server(
        responses: Vec<(u16, &'static str)>,
    ) -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}/api", listener.local_addr().unwrap());
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for (status, body) in responses {
                let deadline = Instant::now() + Duration::from_secs(5);
                let mut socket = loop {
                    match listener.accept() {
                        Ok((socket, _)) => break socket,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(Instant::now() < deadline, "missing expected API request");
                            thread::sleep(Duration::from_millis(5));
                        }
                        Err(err) => panic!("accept: {err}"),
                    }
                };
                socket.set_nonblocking(false).unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let mut bytes = Vec::new();
                while !bytes.ends_with(b"\r\n\r\n") {
                    let mut byte = [0];
                    socket.read_exact(&mut byte).unwrap();
                    bytes.push(byte[0]);
                }
                let headers = String::from_utf8(bytes.clone()).unwrap();
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                let mut payload = vec![0; length];
                socket.read_exact(&mut payload).unwrap();
                bytes.extend(payload);
                requests.push(String::from_utf8(bytes).unwrap());
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
            requests
        });
        (url, handle)
    }

    #[test]
    fn account_requests_match_the_site_contract_without_browser_cookies() {
        let favorites = r#"{"result":{"stations":{"add":[1,2,3],"remove":[2]},"date_sync":7693954098517976834}}"#;
        let ok = r#"{"result":{"status":"ok"}}"#;
        let (url, server) = server(vec![
            (
                200,
                r#"{"result":{"user":{"id":42}},"device_code":"test-device-code"}"#,
            ),
            (200, PROFILE),
            (200, favorites),
            (200, ok),
            (200, ok),
        ]);
        let client = Client::with_base_url(url);
        let login = client
            .login("test@example.com", "radio-record-form!pass")
            .unwrap();
        assert_eq!(login.profile.name, "Test User");
        assert!(login.profile.premium);
        assert_eq!(
            client.get_favorites(&login.device_code).unwrap(),
            BTreeSet::from([1, 3])
        );
        client.set_favorite(&login.device_code, 4, true).unwrap();
        client.set_favorite(&login.device_code, 1, false).unwrap();
        let requests = server.join().unwrap();
        assert!(requests[0].starts_with("POST /api/auth "));
        assert!(requests[0].contains("name=\"USER_PASSWORD\"\r\n\r\nradio-record-form!pass"));
        assert!(requests[0].contains("boundary=radio-record-formx"));
        assert!(requests[1].starts_with("GET /api/profile "));
        for request in &requests[1..] {
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("x-device-code: test-device-code")
            );
            assert!(!request.to_ascii_lowercase().contains("cookie:"));
            assert!(!request.contains("radio-record-form!pass"));
        }
        assert!(requests[3].starts_with("POST /api/favorites/station/?id=4 "));
        assert!(requests[4].starts_with("DELETE /api/favorites/station/?id=1 "));
        assert!(requests[4].contains("name=\"_method\"\r\n\r\nDELETE"));
    }

    #[test]
    fn account_errors_and_malformed_snapshots_do_not_become_empty_favorites() {
        for body in [
            r#"{"result":null}"#,
            r#"{"result":{"stations":{"add":null,"remove":[]}}}"#,
            r#"{"result":{"stations":{"add":["1"],"remove":[]}}}"#,
        ] {
            assert!(decode_favorites(&json::parse(body).unwrap()).is_err());
        }
        assert!(decode_profile(&json::parse(r#"{"result":{"id":0}}"#).unwrap()).is_err());
        let (url, server) = server(vec![(
            401,
            r#"{"password":"must-not-leak","device_code":"secret"}"#,
        )]);
        let error = Client::with_base_url(url)
            .get_profile("test-code")
            .unwrap_err()
            .to_string();
        assert!(error.contains("sign in again"));
        assert!(!error.contains("must-not-leak"));
        server.join().unwrap();
    }

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
        let streams = station.streams();
        assert_eq!(
            streams
                .iter()
                .map(|stream| stream.url.as_str())
                .collect::<Vec<_>>(),
            [
                "https://example.com/stream.mp3",
                "https://example.com/playlist.m3u8",
                "https://example.com/stream64.aacp",
            ]
        );
        assert_eq!(streams[0].kind, StreamKind::Main);
        assert_eq!(streams[1].kind, StreamKind::Hls);
        // The duplicate 128k URL was skipped, so the low stream follows HLS.
        assert_eq!(streams[2].kind, StreamKind::Low);
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
