use crate::model::Playback;
#[cfg(any(not(target_os = "windows"), test))]
use crate::model::{hash, Track};
use std::future::Future;
#[cfg(target_os = "macos")]
use std::time::Duration;

#[cfg(target_os = "windows")]
#[path = "windows_playback.rs"]
mod windows_playback;
#[cfg(target_os = "windows")]
pub use windows_playback::{control, seek};

pub trait PlaybackProvider {
    fn get_state(&self) -> impl Future<Output = Result<Playback, String>> + Send;
}
pub struct Spotify;

// Numeric character separators preserve commas, quotes and newlines in song metadata.
#[cfg(not(target_os = "windows"))]
const SCRIPT: &str = r#"
if not (application id "com.spotify.client" is running) then return "unavailable"
tell application id "com.spotify.client"
    set s to player state as text
    if s is "stopped" then return "stopped"
    set t to current track
    set trackID to ""
    try
        set trackID to id of t as text
    end try
    set fields to {trackID, name of t, artist of t, album of t, duration of t as text, player position as text, s}
    set AppleScript's text item delimiters to ASCII character 31
    return fields as text
end tell
"#;

#[derive(serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackAction {
    Previous,
    Next,
    Play,
    Pause,
}

#[cfg(any(not(target_os = "windows"), test))]
impl PlaybackAction {
    fn command(&self) -> &'static str {
        match self {
            Self::Previous => "previous track",
            Self::Next => "next track",
            Self::Play => "play",
            Self::Pause => "pause",
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub async fn control(action: PlaybackAction) -> Result<(), String> {
    let script = format!(
        r#"
if not (application id "com.spotify.client" is running) then error "Spotify is not running" number -600
tell application id "com.spotify.client" to {}
"#,
        action.command()
    );
    execute(&script).await.map(|_| ())
}

#[cfg(any(not(target_os = "windows"), test))]
fn seek_command(position_ms: u64, duration_ms: u64) -> Result<String, String> {
    if duration_ms == 0 || position_ms > duration_ms {
        return Err("播放位置超出歌曲範圍".into());
    }
    Ok(format!(
        "set player position to {}.{:03}",
        position_ms / 1000,
        position_ms % 1000
    ))
}

#[cfg(not(target_os = "windows"))]
pub async fn seek(position_ms: u64, duration_ms: u64) -> Result<(), String> {
    let command = seek_command(position_ms, duration_ms)?;
    execute(&format!(r#"
if not (application id "com.spotify.client" is running) then error "Spotify is not running" number -600
tell application id "com.spotify.client" to {command}
"#)).await.map(|_| ())
}

impl PlaybackProvider for Spotify {
    async fn get_state(&self) -> Result<Playback, String> {
        #[cfg(target_os = "windows")]
        return windows_playback::get_state().await;
        #[cfg(not(target_os = "windows"))]
        parse(&execute(SCRIPT).await?)
    }
}

#[cfg(not(target_os = "windows"))]
async fn execute(_script: &str) -> Result<String, String> {
    #[cfg(not(target_os = "macos"))]
    return Err("目前只支援 macOS Spotify 桌面版".into());
    #[cfg(target_os = "macos")]
    {
        let mut cmd = tokio::process::Command::new("/usr/bin/osascript");
        cmd.arg("-e").arg(_script).kill_on_drop(true);
        let output = tokio::time::timeout(Duration::from_secs(4), cmd.output())
            .await
            .map_err(|_| "Spotify 回應逾時，請檢查自動化權限".to_string())?
            .map_err(|_| "無法執行 Spotify 指令".to_string())?;
        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            return Err(if error.contains("-1743") {
                "請允許此 App 控制 Spotify，以讀取播放資訊及操作播放"
            } else if error.contains("-600") {
                "請先開啟 Spotify 桌面版"
            } else {
                "Spotify 指令失敗，請確認桌面版已開啟並允許自動化控制"
            }
            .into());
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(any(not(target_os = "windows"), test))]
fn parse(raw: &str) -> Result<Playback, String> {
    let raw = raw.trim_end_matches(['\n', '\r']);
    if raw == "unavailable" {
        return Ok(Playback::default());
    }
    if raw == "stopped" {
        return Ok(Playback {
            status: "stopped".into(),
            message: Some("播放已停止".into()),
            ..Playback::default()
        });
    }
    let fields: Vec<_> = raw.split('\u{1f}').collect();
    if fields.len() != 7 {
        return Err("Spotify 回傳格式無效".into());
    }
    let duration = fields[4].parse::<u64>().map_err(|_| "歌曲長度無效")?;
    let seconds = fields[5]
        .replace(',', ".")
        .parse::<f64>()
        .map_err(|_| "播放位置無效")?;
    if !seconds.is_finite()
        || seconds < 0.0
        || duration == 0
        || !matches!(fields[6], "playing" | "paused")
    {
        return Err("Spotify 播放資訊無效".into());
    }
    let id = if fields[0].is_empty() {
        hash(&serde_json::to_string(&fields[1..5]).unwrap())
    } else {
        fields[0].into()
    };
    Ok(Playback {
        track: Some(Track {
            id,
            title: fields[1].into(),
            artist: fields[2].into(),
            album: fields[3].into(),
            duration_ms: duration,
        }),
        position_ms: ((seconds * 1000.0) as u64).min(duration),
        is_playing: fields[6] == "playing",
        status: fields[6].into(),
        observed_at_monotonic_ms: 0,
        message: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seek_checks_bounds_and_converts_milliseconds() {
        assert_eq!(
            seek_command(12550, 180000).unwrap(),
            "set player position to 12.550"
        );
        assert_eq!(
            seek_command(0, 180000).unwrap(),
            "set player position to 0.000"
        );
        assert!(seek_command(180001, 180000).is_err());
        assert!(seek_command(0, 0).is_err());
    }
    #[test]
    fn controls_only_allow_supported_spotify_commands() {
        for (input, command) in [
            ("previous", "previous track"),
            ("next", "next track"),
            ("play", "play"),
            ("pause", "pause"),
        ] {
            let action: PlaybackAction = serde_json::from_value(serde_json::json!(input)).unwrap();
            assert_eq!(action.command(), command);
        }
        assert!(serde_json::from_value::<PlaybackAction>(serde_json::json!("quit")).is_err());
        assert!(serde_json::from_value::<PlaybackAction>(serde_json::json!("play\nquit")).is_err());
    }

    #[test]
    fn parses_metadata_units_and_unavailable() {
        let raw = [
            "",
            "A, \"new\" day\nHere",
            "Test artist",
            "Album",
            "180000",
            "12.5",
            "paused",
        ]
        .join("\u{1f}");
        let p = parse(&raw).unwrap();
        assert_eq!(p.position_ms, 12500);
        assert_eq!(p.track.unwrap().duration_ms, 180000);
        assert!(!p.is_playing);
        assert_eq!(parse("unavailable\n").unwrap().status, "unavailable");
        assert!(parse(&raw.replace("12.5", "NaN")).is_err());
        assert!(parse("garbage").is_err());
    }
}
