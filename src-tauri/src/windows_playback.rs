use super::PlaybackAction;
use crate::model::{hash, Playback, Track};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession as Session,
    GlobalSystemMediaTransportControlsSessionManager as SessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as Status,
};

const TICKS_PER_MS: i64 = 10_000;
const WINDOWS_EPOCH_OFFSET_MS: i64 = 11_644_473_600_000;
const TIMEOUT: Duration = Duration::from_secs(4);

async fn spotify_session() -> windows::core::Result<Option<Session>> {
    let sessions = SessionManager::RequestAsync()?.await?.GetSessions()?;
    let mut executable_session = None;
    for index in 0..sessions.Size()? {
        let session = sessions.GetAt(index)?;
        let source = session.SourceAppUserModelId()?.to_string();
        if source
            .to_ascii_lowercase()
            .starts_with("spotifyab.spotifymusic_")
        {
            return Ok(Some(session));
        }
        // GSMTC can identify Spotify by its executable rather than its package.
        if source.eq_ignore_ascii_case("Spotify.exe") {
            executable_session = Some(session);
        }
    }
    Ok(executable_session)
}

async fn observation() -> windows::core::Result<Playback> {
    let Some(session) = spotify_session().await? else {
        return Ok(Playback::default());
    };
    let status = session.GetPlaybackInfo()?.PlaybackStatus()?;
    if status == Status::Stopped {
        return Ok(Playback {
            status: "stopped".into(),
            message: Some("播放已停止".into()),
            ..Playback::default()
        });
    }
    if !matches!(status, Status::Playing | Status::Paused) {
        return Ok(Playback::default());
    }
    let media = session.TryGetMediaPropertiesAsync()?.await?;
    let title = media.Title()?.to_string();
    let timeline = session.GetTimelineProperties()?;
    let start = timeline.StartTime()?.Duration;
    let duration = timeline.EndTime()?.Duration.saturating_sub(start);
    if title.is_empty() || duration < TICKS_PER_MS {
        return Ok(Playback {
            message: Some("等待 Spotify 提供歌曲資訊與播放進度…".into()),
            ..Playback::default()
        });
    }
    let artist = media.Artist()?.to_string();
    let album = media.AlbumTitle()?.to_string();
    let duration_ms = (duration / TICKS_PER_MS) as u64;
    let mut position = timeline.Position()?.Duration.saturating_sub(start) / TICKS_PER_MS;
    let is_playing = status == Status::Playing;
    // Position is measured at LastUpdatedTime, which is a Windows UTC timestamp.
    let updated = timeline.LastUpdatedTime()?.UniversalTime / TICKS_PER_MS;
    if is_playing && updated > WINDOWS_EPOCH_OFFSET_MS {
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
        let now_ms = now.as_millis() as i64 + WINDOWS_EPOCH_OFFSET_MS;
        position = position.saturating_add(now_ms.saturating_sub(updated).max(0));
    }
    Ok(Playback {
        track: Some(Track {
            id: hash(&serde_json::json!([title, artist, album, duration_ms]).to_string()),
            title,
            artist,
            album,
            duration_ms,
        }),
        position_ms: (position.max(0) as u64).min(duration_ms),
        is_playing,
        status: if is_playing { "playing" } else { "paused" }.into(),
        observed_at_monotonic_ms: 0,
        message: None,
    })
}

pub async fn get_state() -> Result<Playback, String> {
    tokio::time::timeout(TIMEOUT, observation())
        .await
        .map_err(|_| "Spotify 回報播放狀態逾時".to_string())?
        .map_err(|_| "無法讀取 Windows Spotify 播放資訊".into())
}

async fn active_session() -> Result<Session, String> {
    spotify_session()
        .await
        .map_err(|_| "無法連接 Windows 媒體工作階段".to_string())?
        .ok_or_else(|| "請在 Microsoft Store 版 Spotify 播放一首歌曲".into())
}

pub async fn control(action: PlaybackAction) -> Result<(), String> {
    tokio::time::timeout(TIMEOUT, async {
        let session = active_session().await?;
        let operation = match action {
            PlaybackAction::Previous => session.TrySkipPreviousAsync(),
            PlaybackAction::Next => session.TrySkipNextAsync(),
            PlaybackAction::Play => session.TryPlayAsync(),
            PlaybackAction::Pause => session.TryPauseAsync(),
        };
        let accepted = operation
            .map_err(|_| "無法送出 Spotify 播放指令")?
            .await
            .map_err(|_| "Spotify 播放指令失敗")?;
        accepted
            .then_some(())
            .ok_or_else(|| "Spotify 拒絕播放指令".into())
    })
    .await
    .map_err(|_| "Spotify 播放控制逾時".to_string())?
}

pub async fn seek(position_ms: u64, duration_ms: u64) -> Result<(), String> {
    if duration_ms == 0 || position_ms > duration_ms {
        return Err("播放位置超出歌曲範圍".into());
    }
    let offset = i64::try_from(position_ms)
        .ok()
        .and_then(|position| position.checked_mul(TICKS_PER_MS))
        .ok_or("播放位置超出歌曲範圍")?;
    tokio::time::timeout(TIMEOUT, async {
        let session = active_session().await?;
        let timeline = session
            .GetTimelineProperties()
            .map_err(|_| "無法讀取 Spotify 時間軸")?;
        let start = timeline
            .StartTime()
            .map_err(|_| "無法讀取 Spotify 播放起點")?;
        let position = start
            .Duration
            .checked_add(offset)
            .ok_or("播放位置超出歌曲範圍")?;
        let accepted = session
            .TryChangePlaybackPositionAsync(position)
            .map_err(|_| "無法送出 Spotify 跳轉指令")?
            .await
            .map_err(|_| "Spotify 跳轉失敗")?;
        accepted.then_some(()).ok_or_else(|| "Spotify 拒絕跳轉".into())
    })
    .await
    .map_err(|_| "Spotify 跳轉逾時".to_string())?
}
