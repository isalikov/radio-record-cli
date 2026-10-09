//! A minimal HLS client for Radio Record's playlists: pick the AAC-LC
//! variant from the master playlist, parse media playlists, resolve relative
//! segment URIs, and strip ID3v2 headers from segments. This handles the
//! unencrypted ADTS playlists used by Record; metadata tags are ignored.

use crate::error::{Error, Result};

pub(crate) struct MediaPlaylist {
    pub(crate) target_duration: f64,
    pub(crate) media_sequence: u64,
    pub(crate) segments: Vec<Segment>,
}

pub(crate) struct Segment {
    pub(crate) seq: u64,
    pub(crate) uri: String,
    pub(crate) duration: f64,
}

/// Picks the highest-bandwidth AAC-LC variant (`CODECS="mp4a.40.2"`) from a
/// master playlist, resolved against the playlist URL. HE-AAC variants
/// (`mp4a.40.5`, `mp4a.40.29`) are skipped: this engine only decodes LC.
pub(crate) fn pick_lc_variant(master: &str, base_url: &str) -> Option<String> {
    let mut best: Option<(u64, String)> = None;
    let mut pending_bandwidth: Option<u64> = None;
    for line in master.lines().map(str::trim) {
        if let Some(attributes) = line.strip_prefix("#EXT-X-STREAM-INF:") {
            pending_bandwidth = attribute(attributes, "CODECS")
                .is_some_and(|codecs| codecs.split(',').any(|codec| codec.trim() == "mp4a.40.2"))
                .then(|| {
                    attribute(attributes, "BANDWIDTH")
                        .and_then(|value| value.parse::<u64>().ok())
                        .unwrap_or(0)
                });
        } else if !line.is_empty()
            && !line.starts_with('#')
            && let Some(bandwidth) = pending_bandwidth.take()
            && best
                .as_ref()
                .is_none_or(|(best_bandwidth, _)| bandwidth >= *best_bandwidth)
        {
            best = Some((bandwidth, line.to_owned()));
        }
    }
    best.map(|(_, uri)| resolve(&uri, base_url))
}

pub(crate) fn parse_media(text: &str) -> Result<MediaPlaylist> {
    if text.lines().next().map(str::trim) != Some("#EXTM3U") {
        return Err(Error::new("missing HLS playlist header"));
    }
    let mut target_duration = 6.0;
    let mut media_sequence = 0;
    let mut segments: Vec<Segment> = Vec::new();
    let mut duration: f64 = 0.0;
    for line in text.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            duration = rest
                .split(',')
                .next()
                .unwrap_or("0")
                .trim()
                .parse()
                .map_err(|_| Error::new("invalid HLS segment duration"))?;
            if !duration.is_finite() || duration <= 0.0 {
                return Err(Error::new("invalid HLS segment duration"));
            }
        } else if let Some(rest) = line.strip_prefix("#EXT-X-TARGETDURATION:") {
            let seconds = rest
                .trim()
                .parse::<u32>()
                .map_err(|_| Error::new("invalid HLS target duration"))?;
            if seconds == 0 {
                return Err(Error::new("invalid HLS target duration"));
            }
            target_duration = f64::from(seconds);
        } else if let Some(rest) = line.strip_prefix("#EXT-X-MEDIA-SEQUENCE:") {
            media_sequence = rest
                .trim()
                .parse::<u64>()
                .map_err(|_| Error::new("invalid HLS media sequence"))?;
        } else if !line.is_empty() && !line.starts_with('#') {
            if duration == 0.0 {
                return Err(Error::new("missing HLS segment duration"));
            }
            let next = media_sequence
                .checked_add(segments.len() as u64 + 1)
                .ok_or_else(|| Error::new("HLS media sequence overflow"))?;
            segments.push(Segment {
                seq: next - 1,
                uri: line.to_owned(),
                duration,
            });
            duration = 0.0;
        }
    }
    if segments.is_empty() {
        return Err(Error::new("no segments in HLS playlist"));
    }
    Ok(MediaPlaylist {
        target_duration,
        media_sequence,
        segments,
    })
}

/// Resolve relative paths, root paths, and query strings against the playlist.
pub(crate) fn resolve(uri: &str, base: &str) -> String {
    url::Url::parse(base)
        .and_then(|base| base.join(uri))
        .map_or_else(|_| uri.to_owned(), |url| url.to_string())
}

/// Strips an ID3v2 header (syncsafe size, optional footer) from a segment.
pub(crate) fn strip_id3(bytes: &[u8]) -> &[u8] {
    if bytes.len() >= 10 && &bytes[..3] == b"ID3" {
        let syncsafe = |bytes: &[u8]| {
            (usize::from(bytes[0]) << 21)
                | (usize::from(bytes[1]) << 14)
                | (usize::from(bytes[2]) << 7)
                | usize::from(bytes[3])
        };
        let size = syncsafe(&bytes[6..10]);
        let header = 10 + usize::from(bytes[5] & 0x10 != 0) * 10;
        let end = (header + size).min(bytes.len());
        return &bytes[end..];
    }
    bytes
}

fn attribute(line: &str, key: &str) -> Option<String> {
    let mut quoted = false;
    line.split(|ch| {
        if ch == '"' {
            quoted = !quoted;
        }
        ch == ',' && !quoted
    })
    .find_map(|part| {
        let (name, value) = part.split_once('=')?;
        (name.trim() == key).then(|| value.trim().trim_matches('"').to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap()
    }

    #[test]
    fn picks_the_lc_variant_from_the_real_master_playlist() {
        let master = fixture("hls-master.m3u8");
        let url = pick_lc_variant(&master, "https://hls.example.com/record/playlist.m3u8").unwrap();
        assert_eq!(
            url,
            "https://hls.example.com/record/112/playlist.m3u8?hlssid=dac449d34b3342eaa2ba92ee228cbaca"
        );
        // A master without an LC variant must be refused.
        let he_only = master.replace("mp4a.40.2", "mp4a.40.5");
        assert!(pick_lc_variant(&he_only, "https://h.example/x.m3u8").is_none());
    }

    #[test]
    fn parses_the_real_media_playlist() {
        let playlist = parse_media(&fixture("hls-media.m3u8")).unwrap();
        assert_eq!(playlist.media_sequence, 0);
        assert!(playlist.target_duration >= 5.0);
        assert!(!playlist.segments.is_empty());
        // Sequence numbers must be assigned in order from MEDIA-SEQUENCE.
        assert_eq!(playlist.segments[0].seq, 0);
        assert_eq!(playlist.segments[1].seq, playlist.segments[0].seq + 1);
        assert!(
            playlist
                .segments
                .iter()
                .all(|segment| segment.uri.ends_with(".aac"))
        );
        assert!(
            playlist
                .segments
                .iter()
                .any(|segment| segment.duration > 1.0)
        );
    }

    #[test]
    fn media_playlist_without_segments_is_an_error() {
        assert!(parse_media("#EXTM3U\n#EXT-X-TARGETDURATION:6\n").is_err());
    }

    #[test]
    fn resolves_relative_and_absolute_uris() {
        let base = "https://hls.example.com/record/112/playlist.m3u8?hlssid=abc";
        assert_eq!(
            resolve("l1_segment.aac", base),
            "https://hls.example.com/record/112/l1_segment.aac"
        );
        assert_eq!(
            resolve("https://other.example.com/x.aac", base),
            "https://other.example.com/x.aac"
        );
        assert_eq!(
            resolve("seg.aac", "fixture://112/playlist.m3u8?hlssid=abc"),
            "fixture://112/seg.aac"
        );
        assert_eq!(
            resolve("/live/seg.aac", base),
            "https://hls.example.com/live/seg.aac"
        );
        assert_eq!(
            resolve("../seg.aac", base),
            "https://hls.example.com/record/seg.aac"
        );
        assert_eq!(
            resolve("//cdn.example.com/seg.aac", base),
            "https://cdn.example.com/seg.aac"
        );
        assert_eq!(
            resolve("?token=new", base),
            "https://hls.example.com/record/112/playlist.m3u8?token=new"
        );
    }

    #[test]
    fn master_attributes_keep_quoted_codec_lists_and_comments() {
        let master = "#EXTM3U\n#EXT-X-STREAM-INF:CODECS=\"avc1.42e01e,mp4a.40.2\",BANDWIDTH=112000\n\n#comment\nlow.m3u8\n#EXT-X-STREAM-INF:CODECS=\"mp4a.40.2\",BANDWIDTH=128000\nhigh.m3u8";
        assert_eq!(
            pick_lc_variant(master, "https://example.com/live/master.m3u8"),
            Some("https://example.com/live/high.m3u8".into())
        );
        assert!(
            pick_lc_variant(
                &master.replace("mp4a.40.2", "mp4a.40.29"),
                "https://example.com/master.m3u8"
            )
            .is_none()
        );
    }

    #[test]
    fn invalid_durations_and_sequences_are_errors_instead_of_panics() {
        for tag in [
            "#EXT-X-TARGETDURATION:NaN",
            "#EXT-X-TARGETDURATION:inf",
            "#EXT-X-TARGETDURATION:0",
            "#EXTINF:NaN",
            "#EXTINF:-1",
            "#EXT-X-MEDIA-SEQUENCE:18446744073709551615",
        ] {
            let text = format!("#EXTM3U\n#EXTINF:6\n{tag}\nsegment.aac");
            assert!(parse_media(&text).is_err(), "accepted {tag}");
        }
        assert!(parse_media("#EXTM3U\nsegment.aac").is_err());
        assert!(parse_media("not a playlist").is_err());
    }

    #[test]
    fn strips_id3_headers() {
        let segment = include_bytes!("../../tests/fixtures/112/l0_6ac775da60ecdd612e0db32e.aac");
        let stripped = strip_id3(segment);
        assert!(
            stripped.starts_with(&[0xff, 0xf1]),
            "ADTS must follow the ID3 header"
        );
        assert!(stripped.len() < segment.len());
        assert_eq!(strip_id3(stripped).as_ptr(), stripped.as_ptr());
    }
}
