//! Saved songs (the track vault): the record kept in settings, the links to
//! streaming services built from it, the optional direct Yandex Music lookup,
//! and the side effects behind them (opening a browser, copying a link).

use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    process::{Command, Stdio},
    time::Duration,
};

use crate::{
    api::Track,
    error::{Error, Result},
    json::{self, JsonValue},
};

const YANDEX_SEARCH: &str = "https://api.music.yandex.net/search";

/// One liked song, as heard on air. Only artist and song are required; the
/// links are whatever Radio Record and the Yandex lookup provided.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Song {
    /// Radio Record track id; 0 when unknown.
    pub id: i64,
    pub artist: String,
    pub song: String,
    /// The station it was heard on.
    pub station: String,
    /// When it was saved (Unix seconds).
    pub saved: i64,
    /// Direct Apple Music link from the Radio Record API.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub itunes_url: String,
    /// The radiorecord.ru track page.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub share_url: String,
    /// Direct Yandex Music link, once a lookup found one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub yandex_url: String,
}

impl Song {
    pub fn from_track(track: &Track, station: &str, saved: i64) -> Self {
        Self {
            id: track.id,
            artist: track.artist.trim().to_owned(),
            song: track.song.trim().to_owned(),
            station: station.to_owned(),
            saved,
            itunes_url: track.itunes_url.clone(),
            share_url: track.share_url.clone(),
            yandex_url: String::new(),
        }
    }

    /// Identity for duplicates: case- and spacing-insensitive artist and song.
    pub fn key(&self) -> String {
        key(&self.artist, &self.song)
    }

    /// `Artist — Song`, or whichever half exists.
    pub fn label(&self) -> String {
        match (self.artist.is_empty(), self.song.is_empty()) {
            (false, false) => format!("{} — {}", self.artist, self.song),
            (true, _) => self.song.clone(),
            (_, true) => self.artist.clone(),
        }
    }

    /// The search text used for every service: artist and song without
    /// radio decorations, the way radiorecord.ru builds its own links.
    pub fn query(&self) -> String {
        clean(&format!("{} {}", self.artist, self.song))
    }

    /// Links in display order. Search links always exist; direct links
    /// replace them where known.
    pub fn links(&self) -> Vec<Link> {
        let query = encode(&self.query());
        let mut links = vec![
            Link {
                service: Service::YouTube,
                url: format!("https://music.youtube.com/search?q={query}"),
                direct: false,
            },
            Link {
                service: Service::Spotify,
                url: format!("https://open.spotify.com/search/{query}"),
                direct: false,
            },
            if self.yandex_url.is_empty() {
                Link {
                    service: Service::Yandex,
                    url: format!("https://music.yandex.ru/search?text={query}&type=tracks"),
                    direct: false,
                }
            } else {
                Link {
                    service: Service::Yandex,
                    url: self.yandex_url.clone(),
                    direct: true,
                }
            },
            if self.itunes_url.is_empty() {
                Link {
                    service: Service::Apple,
                    url: format!("https://music.apple.com/search?term={query}"),
                    direct: false,
                }
            } else {
                Link {
                    service: Service::Apple,
                    url: self.itunes_url.clone(),
                    direct: true,
                }
            },
        ];
        if self.share_url.starts_with("https://") {
            links.push(Link {
                service: Service::Record,
                url: self.share_url.clone(),
                direct: true,
            });
        }
        links
    }
}

pub fn key(artist: &str, song: &str) -> String {
    let words = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    format!("{}|{}", words(artist), words(song)).to_lowercase()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    YouTube,
    Spotify,
    Yandex,
    Apple,
    Record,
}

impl Service {
    pub fn name(self) -> &'static str {
        match self {
            Self::YouTube => "YouTube Music",
            Self::Spotify => "Spotify",
            Self::Yandex => "Yandex Music",
            Self::Apple => "Apple Music",
            Self::Record => "Radio Record",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub service: Service,
    pub url: String,
    /// A link to the track itself rather than a search for it.
    pub direct: bool,
}

/// Drop what radio adds to titles: bracketed tags, premiere banners, and
/// separators that confuse search engines.
fn clean(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for character in text.chars() {
        match character {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            '/' | '\\' | '\'' | '!' if depth == 0 => out.push(' '),
            _ if depth == 0 => out.push(character),
            _ => {}
        }
    }
    out.replace("ПРЕМЬЕРА", " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

/// Percent-encode for both query strings and path segments (spaces as %20).
fn encode(text: &str) -> String {
    // byte_serialize writes a literal '+' as %2B, so every '+' left is a space.
    url::form_urlencoded::byte_serialize(text.as_bytes())
        .collect::<String>()
        .replace('+', "%20")
}

/// Ask the unofficial Yandex Music search for the song and return a direct
/// track link when the top results clearly match. `Ok(None)` means no match.
pub fn lookup_yandex(song: &Song) -> Result<Option<String>> {
    let http: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(crate::api::USER_AGENT)
        .timeout_connect(Some(Duration::from_secs(5)))
        .timeout_global(Some(Duration::from_secs(8)))
        .build()
        .into();
    let url = format!(
        "{YANDEX_SEARCH}?text={}&type=track&page=0",
        encode(&song.query())
    );
    let response = http
        .get(&url)
        .call()
        .map_err(|_| Error::new("Yandex Music unavailable"))?;
    let mut body = String::new();
    response
        .into_body()
        .as_reader()
        .take(4 * 1024 * 1024)
        .read_to_string(&mut body)
        .map_err(|_| Error::new("Yandex Music unavailable"))?;
    Ok(decode_yandex(&json::parse(&body)?, song))
}

fn decode_yandex(value: &JsonValue, song: &Song) -> Option<String> {
    let results = value
        .object_get("result")?
        .object_get("tracks")?
        .object_get("results")?
        .as_array()?;
    let fold = |text: &str| clean(text).to_lowercase();
    let wanted_title = fold(&song.song);
    let wanted_artist = fold(&song.artist);
    if wanted_title.is_empty() {
        return None;
    }
    let id = |value: &JsonValue| {
        value
            .as_i64()
            .map(|id| id.to_string())
            .or_else(|| value.as_str().map(str::to_owned))
            .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    };
    results.iter().take(5).find_map(|track| {
        let title = fold(track.object_get("title")?.as_str()?);
        let artists = track
            .object_get("artists")?
            .as_array()?
            .iter()
            .filter_map(|artist| artist.object_get("name")?.as_str())
            .map(fold)
            .collect::<Vec<_>>();
        let title_matches = title == wanted_title
            || title.starts_with(&wanted_title)
            || wanted_title.starts_with(&title);
        let artist_matches = wanted_artist.is_empty()
            || artists
                .iter()
                .any(|artist| !artist.is_empty() && wanted_artist.contains(artist.as_str()));
        if !title_matches || !artist_matches {
            return None;
        }
        let track_id = id(track.object_get("id")?)?;
        let album_id = id(track
            .object_get("albums")?
            .as_array()?
            .first()?
            .object_get("id")?)?;
        Some(format!(
            "https://music.yandex.ru/album/{album_id}/track/{track_id}"
        ))
    })
}

/// Open `url` in the default browser without waiting for it, and without
/// letting the launcher write over the screen.
pub fn open(url: &str) -> Result<()> {
    if !url.starts_with("https://") {
        return Err(Error::new("Refusing to open a non-https link"));
    }
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let mut child = Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| Error::new(format!("Could not run {program}")))?;
    // Reap the launcher off the main thread so it never lingers as a zombie.
    std::thread::spawn(move || child.wait());
    Ok(())
}

/// Put `text` on the clipboard through the terminal (OSC 52). Terminals that
/// do not support it ignore the sequence.
pub fn copy(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    write!(stdout, "\x1b]52;c;{}\x07", base64(text.as_bytes()))?;
    stdout.flush()?;
    Ok(())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, b)| n | u32::from(*b) << (16 - 8 * i));
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                TABLE[(n >> (18 - 6 * i) & 63) as usize] as char
            } else {
                '='
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(artist: &str, title: &str) -> Song {
        Song {
            artist: artist.into(),
            song: title.into(),
            ..Song::default()
        }
    }

    #[test]
    fn search_links_strip_radio_decorations_and_encode_cyrillic() {
        let decorated = song(
            "SINISTER SOULS/KAIZEN FLOW",
            "Charged (Record Mix) [Premiere]",
        );
        assert_eq!(decorated.query(), "SINISTER SOULS KAIZEN FLOW Charged");
        let links = decorated.links();
        assert_eq!(
            links[0].url,
            "https://music.youtube.com/search?q=SINISTER%20SOULS%20KAIZEN%20FLOW%20Charged"
        );
        assert_eq!(
            links[1].url,
            "https://open.spotify.com/search/SINISTER%20SOULS%20KAIZEN%20FLOW%20Charged"
        );
        assert!(links.iter().all(|link| !link.direct));
        assert_eq!(links.len(), 4, "no Radio Record page without a share link");
        let cyrillic = song("Ласковый Май", "Белые розы !! ПРЕМЬЕРА !!!");
        assert_eq!(cyrillic.query(), "Ласковый Май Белые розы");
        assert_eq!(
            cyrillic.links()[2].url,
            "https://music.yandex.ru/search?text=%D0%9B%D0%B0%D1%81%D0%BA%D0%BE%D0%B2%D1%8B%D0%B9%20%D0%9C%D0%B0%D0%B9%20%D0%91%D0%B5%D0%BB%D1%8B%D0%B5%20%D1%80%D0%BE%D0%B7%D1%8B&type=tracks"
        );
        assert_eq!(song("A+B", "C&D").query(), "A+B C&D");
        assert!(song("A+B", "C&D").links()[0].url.ends_with("A%2BB%20C%26D"));
    }

    #[test]
    fn direct_links_replace_searches_when_known() {
        let mut song = song("Artist", "Song");
        song.itunes_url = "https://music.apple.com/us/album/x/1?i=2".into();
        song.share_url = "https://www.radiorecord.ru/track/5/".into();
        song.yandex_url = "https://music.yandex.ru/album/1/track/2".into();
        let links = song.links();
        assert_eq!(links.len(), 5);
        assert!(links[2..].iter().all(|link| link.direct));
        assert_eq!(links[4].service, Service::Record);
    }

    #[test]
    fn keys_ignore_case_and_spacing() {
        assert_eq!(
            key(" Armin  van Buuren", "Blue Fear "),
            key("armin van buuren", "BLUE FEAR")
        );
        assert_ne!(key("A", "B"), key("A", "C"));
    }

    #[test]
    fn yandex_lookup_accepts_only_a_matching_track() {
        let body = r#"{"result":{"tracks":{"results":[
            {"id":1,"title":"Other","artists":[{"name":"Armin van Buuren"}],"albums":[{"id":9}]},
            {"id":"35225863","title":"Blue Fear","artists":[{"name":"Armin van Buuren"}],"albums":[{"id":4563787}]}
        ]}}}"#;
        let value = json::parse(body).unwrap();
        assert_eq!(
            decode_yandex(&value, &song("Armin van Buuren", "Blue Fear (Radio Edit)")).as_deref(),
            Some("https://music.yandex.ru/album/4563787/track/35225863")
        );
        assert_eq!(
            decode_yandex(&value, &song("Someone Else", "Blue Fear")),
            None
        );
        assert_eq!(
            decode_yandex(&json::parse("{}").unwrap(), &song("A", "B")),
            None
        );
    }

    #[test]
    fn base64_matches_the_standard_alphabet() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64("https://ü".as_bytes()), "aHR0cHM6Ly/DvA==");
    }

    #[test]
    #[ignore = "requires network access to Yandex Music"]
    fn live_yandex_lookup_finds_a_known_track() {
        let url = lookup_yandex(&song("Armin van Buuren", "Blue Fear")).unwrap();
        assert!(url.unwrap().starts_with("https://music.yandex.ru/album/"));
    }

    #[test]
    fn only_https_links_are_opened() {
        assert!(open("file:///etc/passwd").is_err());
        assert!(open("javascript:alert(1)").is_err());
    }
}
