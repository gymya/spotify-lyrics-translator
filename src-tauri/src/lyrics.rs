use crate::model::{Candidate, Line, Lyrics, Track};
use serde::Deserialize;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Mutex;

pub struct Api {
    pub client: reqwest::Client,
    pub lyrics_after: Mutex<Instant>,
    pub translation_after: Mutex<Instant>,
}
impl Api {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(
                    "SpotifyLyricsTranslator/0.1.0 (macOS; app.spotify-lyrics-translator.desktop)",
                )
                .timeout(Duration::from_secs(25))
                .build()?,
            lyrics_after: Mutex::new(Instant::now()),
            translation_after: Mutex::new(Instant::now()),
        })
    }
}

pub async fn check_status(
    response: &reqwest::Response,
    gate: &Mutex<Instant>,
) -> Result<(), String> {
    if response.status().is_success() {
        return Ok(());
    }
    if response.status().as_u16() == 429 || response.status().as_u16() == 503 {
        let delay = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| {
                v.parse::<u64>().ok().map(Duration::from_secs).or_else(|| {
                    httpdate::parse_http_date(v)
                        .ok()
                        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
                })
            })
            .unwrap_or(Duration::from_secs(60));
        *gate.lock().await = Instant::now()
            .checked_add(delay)
            .unwrap_or(Instant::now() + Duration::from_secs(86400));
        return Err(format!("服務暫時忙碌，請於 {} 秒後重試", delay.as_secs()));
    }
    Err(format!(
        "API 請求失敗（HTTP {}）",
        response.status().as_u16()
    ))
}
pub async fn reserve(gate: &Mutex<Instant>) -> Result<(), String> {
    let mut next = gate.lock().await;
    if *next > Instant::now() {
        return Err(format!(
            "請於 {} 秒後重試",
            next.duration_since(Instant::now()).as_secs() + 1
        ));
    }
    *next = Instant::now() + Duration::from_secs(1);
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: u64,
    track_name: String,
    artist_name: String,
    album_name: String,
    duration: f64,
    instrumental: bool,
    synced_lyrics: Option<String>,
    plain_lyrics: Option<String>,
}
fn normalized(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
fn matching(record: &Record, track: &Track) -> bool {
    normalized(&record.track_name) == normalized(&track.title)
        && normalized(&record.artist_name) == normalized(&track.artist)
        && (track.album.is_empty() || normalized(&record.album_name) == normalized(&track.album))
        && record.duration.is_finite()
        && (record.duration * 1000.0 - track.duration_ms as f64).abs() <= 2000.0
}
pub async fn fetch(api: &Api, track: &Track) -> Result<Lyrics, String> {
    reserve(&api.lyrics_after).await?;
    let response = api
        .client
        .get("https://lrclib.net/api/get")
        .query(&[
            ("track_name", track.title.clone()),
            ("artist_name", track.artist.clone()),
            ("album_name", track.album.clone()),
            ("duration", (track.duration_ms as f64 / 1000.0).to_string()),
        ])
        .send()
        .await
        .map_err(|_| "歌詞網路連線失敗，請稍後再試")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return search_fallback(api, track).await;
    }
    check_status(&response, &api.lyrics_after).await?;
    let record: Record = response.json().await.map_err(|_| "歌詞服務回傳格式無效")?;
    if !matching(&record, track) {
        return search_fallback(api, track).await;
    }
    Ok(record_lyrics(record))
}

fn record_lyrics(record: Record) -> Lyrics {
    if record.instrumental {
        return Lyrics::Instrumental;
    }
    if let Some(raw) = record.synced_lyrics {
        let lines = parse_lrc(&raw);
        if !lines.is_empty() {
            return Lyrics::Synced { lines };
        }
    }
    match record.plain_lyrics.filter(|s| !s.trim().is_empty()) {
        Some(text) => Lyrics::Unsynced { text },
        None => Lyrics::NotFound,
    }
}

async fn search_fallback(api: &Api, track: &Track) -> Result<Lyrics, String> {
    tokio::time::sleep(Duration::from_secs(1)).await;
    search(api, track).await
}

pub async fn search(api: &Api, track: &Track) -> Result<Lyrics, String> {
    reserve(&api.lyrics_after).await?;
    let response = api
        .client
        .get("https://lrclib.net/api/search")
        .query(&[("track_name", &track.title)])
        .send()
        .await
        .map_err(|_| "歌詞搜尋連線失敗，請稍後再試")?;
    check_status(&response, &api.lyrics_after).await?;
    let records: Vec<Record> = response.json().await.map_err(|_| "歌詞搜尋回傳格式無效")?;
    Ok(candidates(records, track))
}

fn candidates(records: Vec<Record>, track: &Track) -> Lyrics {
    let mut choices = Vec::new();
    for record in records {
        if !record.duration.is_finite()
            || record.duration <= 0.0
            || (record.duration * 1000.0 - track.duration_ms as f64).abs() > 2000.0
        {
            continue;
        }
        let mut candidate = Candidate {
            id: record.id,
            title: record.track_name.clone(),
            artist: record.artist_name.clone(),
            album: record.album_name.clone(),
            duration_ms: (record.duration * 1000.0).round() as u64,
            kind: String::new(),
            lyrics: None,
        };
        let lyrics = record_lyrics(record);
        candidate.kind = match &lyrics {
            Lyrics::Synced { .. } => "synced",
            _ => continue,
        }
        .into();
        candidate.lyrics = Some(Box::new(lyrics));
        choices.push(candidate);
    }
    choices.sort_by_key(|c| {
        (
            normalized(&c.title) != normalized(&track.title),
            normalized(&c.album) != normalized(&track.album),
            c.duration_ms.abs_diff(track.duration_ms),
            c.id,
        )
    });
    if choices.is_empty() {
        Lyrics::NotFound
    } else {
        Lyrics::Candidates {
            candidates: choices,
        }
    }
}

fn timestamp(tag: &str) -> Option<i64> {
    let (minutes, seconds) = tag.split_once(':')?;
    let (seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    if minutes.is_empty()
        || seconds.is_empty()
        || !minutes.bytes().all(|b| b.is_ascii_digit())
        || !seconds.bytes().all(|b| b.is_ascii_digit())
        || !fraction.bytes().all(|b| b.is_ascii_digit())
        || fraction.len() > 3
    {
        return None;
    }
    let minutes = minutes.parse::<i64>().ok()?;
    let seconds = seconds.parse::<i64>().ok()?;
    if seconds >= 60 {
        return None;
    }
    let milliseconds = if fraction.is_empty() {
        0
    } else {
        fraction.parse::<i64>().ok()? * 10_i64.pow(3 - fraction.len() as u32)
    };
    minutes
        .checked_mul(60000)?
        .checked_add(seconds * 1000 + milliseconds)
}

pub fn parse_lrc(raw: &str) -> Vec<Line> {
    let mut lines = Vec::new();
    let mut offset = 0i64;
    for source in raw.lines() {
        let mut rest = source.trim();
        let mut times = Vec::new();
        while let Some(tagged) = rest.strip_prefix('[') {
            let Some((tag, after)) = tagged.split_once(']') else {
                break;
            };
            if let Some(value) = tag
                .strip_prefix("offset:")
                .and_then(|s| s.parse::<i64>().ok())
            {
                offset = value.clamp(-60000, 60000);
            }
            if let Some(time) = timestamp(tag) {
                times.push(time);
            }
            rest = after;
        }
        for time in times {
            lines.push(Line {
                start_time_ms: time,
                original: rest.trim().into(),
                translated: None,
            });
        }
    }
    for line in &mut lines {
        line.start_time_ms = line.start_time_ms.saturating_add(offset).max(0);
    }
    lines.sort_by_key(|line| line.start_time_ms);
    let mut merged: Vec<Line> = Vec::new();
    for line in lines {
        if let Some(previous) = merged
            .last_mut()
            .filter(|p| p.start_time_ms == line.start_time_ms)
        {
            if previous.original != line.original && !line.original.is_empty() {
                if !previous.original.is_empty() {
                    previous.original.push_str(" / ");
                }
                previous.original.push_str(&line.original);
            }
        } else {
            merged.push(line);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lrc_metadata_multiple_tags_duplicates_and_invalid_times() {
        let lines = parse_lrc("[ar:Test]\n[00:02.123][00:01.50]Morning\n[00:01.500]Light\n[00:03.00]\n[00:99.1]bad\n[xx:yy]bad\n[999999999999999999:00]bad\n[offset:-100]");
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].start_time_ms, 1400);
        assert_eq!(lines[0].original, "Morning / Light");
        assert_eq!(lines[1].start_time_ms, 2023);
        assert_eq!(lines[2].original, "");
    }
    #[test]
    fn search_allows_artist_aliases_but_requires_confirmation_and_close_duration() {
        let track = Track {
            id: "spotify:1".into(),
            title: "朝".into(),
            artist: "日本語の歌手".into(),
            album: "Single".into(),
            duration_ms: 180000,
        };
        let record = |id, duration| Record {
            id,
            track_name: "朝".into(),
            artist_name: "Romanized Artist".into(),
            album_name: "Album".into(),
            duration,
            instrumental: false,
            synced_lyrics: Some("[00:01.00]A new morning".into()),
            plain_lyrics: None,
        };
        let mut plain = record(3, 180.0);
        plain.synced_lyrics = None;
        plain.plain_lyrics = Some("Plain lyrics".into());
        let mut instrumental = record(4, 180.0);
        instrumental.instrumental = true;
        let result = candidates(
            vec![record(1, 181.0), record(2, 190.0), plain, instrumental],
            &track,
        );
        let Lyrics::Candidates { candidates } = &result else {
            panic!("must ask user to choose")
        };
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].id, 1);
        assert!(matches!(
            candidates[0].lyrics.as_deref(),
            Some(Lyrics::Synced { .. })
        ));
        let json = serde_json::to_value(&result).unwrap();
        assert!(json["candidates"][0].get("lyrics").is_none());
        assert_eq!(
            self::candidates(vec![record(2, 190.0)], &track),
            Lyrics::NotFound
        );
    }

    #[test]
    fn rejects_wrong_version_and_duration() {
        let track = Track {
            id: "id".into(),
            title: "Morning".into(),
            artist: "Artist".into(),
            album: "Album".into(),
            duration_ms: 180000,
        };
        let mut record = Record {
            id: 1,
            track_name: "morning".into(),
            artist_name: "Artist".into(),
            album_name: "Album".into(),
            duration: 180.0,
            instrumental: false,
            synced_lyrics: None,
            plain_lyrics: None,
        };
        assert!(matching(&record, &track));
        record.track_name = "Morning (Live)".into();
        assert!(!matching(&record, &track));
        record.track_name = "Morning".into();
        record.duration = 185.0;
        assert!(!matching(&record, &track));
    }
}
