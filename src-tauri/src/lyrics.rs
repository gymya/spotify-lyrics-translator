use crate::model::{Candidate, Line, Lyrics, Track};
use reqwest::{header::RETRY_AFTER, Method, RequestBuilder, Response, StatusCode};
use serde::{de::DeserializeOwned, Deserialize};
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::Mutex;

const MAX_RETRIES: usize = 3;
const RETRY_DELAY: Duration = Duration::from_secs(5);
const REQUEST_INTERVAL: Duration = Duration::from_secs(1);

pub enum ApiResponse<T> {
    Success(T),
    HttpError { response: Response, message: String },
}

pub enum RequestKind {
    AutomaticLyrics,
    ManualLyrics,
    Translation,
}

struct RequestGate {
    next_request: Instant,
    server_retry_at: Instant,
}

impl RequestGate {
    fn new() -> Mutex<Self> {
        let now = Instant::now();
        Mutex::new(Self {
            next_request: now,
            server_retry_at: now,
        })
    }
}

pub struct Api {
    pub client: reqwest::Client,
    lyrics_gate: Mutex<RequestGate>,
    translation_gate: Mutex<RequestGate>,
}
impl Api {
    pub fn new() -> Result<Self, reqwest::Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .user_agent(
                    "SpotifyLyricsTranslator/0.1.0 (macOS; app.spotify-lyrics-translator.desktop)",
                )
                .timeout(Duration::from_secs(25))
                .retry(reqwest::retry::never())
                .build()?,
            lyrics_gate: RequestGate::new(),
            translation_gate: RequestGate::new(),
        })
    }

    pub async fn send_json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        kind: RequestKind,
        network_error: &str,
        format_error: &str,
    ) -> Result<ApiResponse<T>, String> {
        let request = request.build().map_err(|_| network_error)?;
        let read_only = request.method() == Method::GET;
        let gate = match kind {
            RequestKind::AutomaticLyrics | RequestKind::ManualLyrics => &self.lyrics_gate,
            RequestKind::Translation => &self.translation_gate,
        };
        let mut remaining = MAX_RETRIES;
        loop {
            reserve(gate, matches!(kind, RequestKind::AutomaticLyrics)).await?;
            let attempt_request = request.try_clone().ok_or(network_error)?;
            match self.client.execute(attempt_request).await {
                Ok(response) if response.status().is_success() => match response.json().await {
                    Ok(value) => return Ok(ApiResponse::Success(value)),
                    Err(error) if error.is_timeout() || error.is_body() => {
                        if !read_only || remaining == 0 {
                            return Err(network_error.into());
                        }
                    }
                    Err(_) => return Err(format_error.into()),
                },
                Ok(response) => {
                    let (message, retry) = http_failure(&response, gate).await;
                    if !retry || remaining == 0 {
                        return Ok(ApiResponse::HttpError { response, message });
                    }
                }
                Err(error) => {
                    // A timed-out translation might already have been processed and billed.
                    let retry = error.is_connect()
                        || (read_only && (error.is_timeout() || error.is_request()));
                    if !retry || remaining == 0 {
                        return Err(network_error.into());
                    }
                }
            }
            remaining -= 1;
            tokio::time::sleep(RETRY_DELAY).await;
        }
    }
}

async fn http_failure(response: &Response, gate: &Mutex<RequestGate>) -> (String, bool) {
    let status = response.status();
    let message = format!("API 請求失敗（HTTP {}）", status.as_u16());
    if !matches!(status.as_u16(), 429 | 500 | 502 | 503 | 504) {
        return (message, false);
    }

    let delay = response
        .headers()
        .get(RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| {
            value
                .parse::<u64>()
                .ok()
                .map(Duration::from_secs)
                .or_else(|| {
                    httpdate::parse_http_date(value)
                        .ok()
                        .map(|date| date.duration_since(SystemTime::now()).unwrap_or_default())
                })
        })
        .or_else(|| (status == StatusCode::TOO_MANY_REQUESTS).then_some(Duration::from_secs(60)));
    let Some(delay) = delay else {
        return (message, true);
    };

    let now = Instant::now();
    let until = now
        .checked_add(delay)
        .unwrap_or(now + Duration::from_secs(86400));
    let mut gate = gate.lock().await;
    gate.server_retry_at = gate.server_retry_at.max(until);
    let seconds = delay.as_secs() + u64::from(delay.subsec_nanos() > 0);
    (
        format!("服務暫時忙碌，請於 {seconds} 秒後重試"),
        delay <= RETRY_DELAY,
    )
}

async fn reserve(gate: &Mutex<RequestGate>, wait_for_interval: bool) -> Result<(), String> {
    loop {
        let mut state = gate.lock().await;
        let now = Instant::now();
        let next = state.next_request.max(state.server_retry_at);
        if next <= now {
            state.next_request = now + REQUEST_INTERVAL;
            return Ok(());
        }
        if !wait_for_interval || state.server_retry_at > now {
            return Err(format!(
                "請於 {} 秒後重試",
                next.duration_since(now).as_secs() + 1
            ));
        }
        drop(state);
        tokio::time::sleep_until(next.into()).await;
    }
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
    let request = api
        .client
        .get("https://lrclib.net/api/get")
        .query(&[
            ("track_name", track.title.clone()),
            ("artist_name", track.artist.clone()),
            ("album_name", track.album.clone()),
            ("duration", (track.duration_ms as f64 / 1000.0).to_string()),
        ]);
    let record: Record = match api
        .send_json(
            request,
            RequestKind::AutomaticLyrics,
            "歌詞網路連線失敗，請稍後再試",
            "歌詞服務回傳格式無效",
        )
        .await?
    {
        ApiResponse::Success(record) => record,
        ApiResponse::HttpError { response, .. } if response.status() == StatusCode::NOT_FOUND => {
            return search(api, track, RequestKind::AutomaticLyrics).await;
        }
        ApiResponse::HttpError { message, .. } => return Err(message),
    };
    if !matching(&record, track) {
        return search(api, track, RequestKind::AutomaticLyrics).await;
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

pub async fn search(api: &Api, track: &Track, kind: RequestKind) -> Result<Lyrics, String> {
    let request = api
        .client
        .get("https://lrclib.net/api/search")
        .query(&[("track_name", &track.title)]);
    match api
        .send_json(
            request,
            kind,
            "歌詞搜尋連線失敗，請稍後再試",
            "歌詞搜尋回傳格式無效",
        )
        .await?
    {
        ApiResponse::Success(records) => Ok(candidates(records, track)),
        ApiResponse::HttpError { message, .. } => Err(message),
    }
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
    use std::{
        io::{BufRead, BufReader, Write},
        net::TcpListener,
        thread,
    };

    fn serve_responses(responses: &[&'static str]) -> (String, thread::JoinHandle<Vec<Instant>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses.to_vec();
        let server = thread::spawn(move || {
            responses
                .into_iter()
                .map(|response| {
                    let (mut stream, _) = listener.accept().unwrap();
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let mut reader = BufReader::new(&stream);
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap() == 0 || line == "\r\n" {
                            break;
                        }
                    }
                    let received = Instant::now();
                    stream.write_all(response.as_bytes()).unwrap();
                    received
                })
                .collect()
        });
        (url, server)
    }

    #[tokio::test]
    async fn automatic_lyrics_wait_for_request_interval_after_cancelled_lookup() {
        let (url, server) = serve_responses(&[
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}";
            2
        ]);
        let api = Api::new().unwrap();
        let lookup = || {
            api.send_json::<serde_json::Value>(
                api.client.get(&url),
                RequestKind::AutomaticLyrics,
                "network",
                "format",
            )
        };
        let started = Instant::now();
        assert!(matches!(lookup().await, Ok(ApiResponse::Success(_))));
        assert!(tokio::time::timeout(Duration::from_millis(25), lookup())
            .await
            .is_err());
        assert!(matches!(lookup().await, Ok(ApiResponse::Success(_))));

        let received = server.join().unwrap();
        assert!(received[1] - started >= REQUEST_INTERVAL);
    }

    #[tokio::test]
    async fn retries_transient_failures_three_times_with_five_second_delays() {
        let (url, server) = serve_responses(&[
            "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n";
            4
        ]);
        let api = Api::new().unwrap();
        let result = api
            .send_json::<serde_json::Value>(
                api.client.get(url),
                RequestKind::AutomaticLyrics,
                "network",
                "format",
            )
            .await;
        assert!(matches!(
            result,
            Ok(ApiResponse::HttpError { response, .. })
                if response.status() == StatusCode::SERVICE_UNAVAILABLE
        ));
        let received = server.join().unwrap();
        assert!(received
            .windows(2)
            .all(|pair| pair[1] - pair[0] >= RETRY_DELAY));
    }

    #[tokio::test]
    async fn returns_rate_limit_with_long_retry_after_and_keeps_the_cooldown() {
        let (url, server) = serve_responses(&[
            "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 30\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ]);
        let api = Api::new().unwrap();
        let result = api
            .send_json::<serde_json::Value>(
                api.client.get(url),
                RequestKind::AutomaticLyrics,
                "network",
                "format",
            )
            .await;
        assert!(matches!(
            result,
            Ok(ApiResponse::HttpError { response, message })
                if response.status() == StatusCode::TOO_MANY_REQUESTS && message.contains("30 秒")
        ));
        assert!(reserve(&api.lyrics_gate, true).await.is_err());
        server.join().unwrap();
    }

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
