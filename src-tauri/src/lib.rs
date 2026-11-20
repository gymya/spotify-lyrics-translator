mod lyrics;
#[cfg(target_os = "macos")]
mod macos;
mod model;
mod playback;
mod storage;
mod sync;
mod translation;

use model::{Lyrics, Playback, Settings, Snapshot, Track, TranslationProvider};
use playback::PlaybackProvider;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Manager, State,
};

struct Runtime {
    playback: Playback,
    observed: Instant,
    lyrics: Option<Lyrics>,
    search_results: Option<Lyrics>,
    loading: bool,
    translating: bool,
    translation_message: Option<String>,
    settings: Settings,
    editing: bool,
    generation: u64,
}
struct AppState {
    data: Mutex<Runtime>,
    db: storage::Storage,
    api: lyrics::Api,
    retry: AtomicU64,
}
type Shared = Arc<AppState>;

fn main_only(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("此操作僅限設定視窗".into());
    }
    Ok(())
}

#[tauri::command]
fn get_snapshot(state: State<'_, Shared>) -> Snapshot {
    let data = state.data.lock().unwrap();
    let mut playback = data.playback.clone();
    if let Some(track) = &playback.track {
        playback.position_ms = sync::estimated_position(
            playback.position_ms,
            data.observed.elapsed().as_millis() as u64,
            playback.is_playing,
            track.duration_ms,
        );
    }
    let line = match &data.lyrics {
        Some(Lyrics::Synced { lines }) => {
            sync::current_line(playback.position_ms, lines, data.settings.offset_ms)
        }
        _ => None,
    };
    Snapshot {
        playback,
        line,
        lyrics: data.lyrics.clone(),
        loading: data.loading,
        translating: data.translating,
        translation_message: data.translation_message.clone(),
        settings: data.settings.clone(),
        editing: data.editing,
    }
}

fn apply_overlay(app: &tauri::AppHandle, settings: &Settings, editing: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    return macos::apply_overlay(app, settings.clone(), editing);
    #[cfg(not(target_os = "macos"))]
    {
        let window = app.get_webview_window("overlay").ok_or("找不到浮動視窗")?;
        window
            .set_always_on_top(settings.always_on_top)
            .map_err(|e| e.to_string())?;
        #[cfg(target_os = "windows")]
        window.set_focusable(editing).map_err(|e| e.to_string())?;
        window
            .set_ignore_cursor_events(settings.click_through && !editing)
            .map_err(|e| e.to_string())?;
        if settings.overlay_enabled {
            window.show()
        } else {
            window.hide()
        }
        .map_err(|e| e.to_string())
    }
}

#[tauri::command]
fn save_settings(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    mut settings: Settings,
) -> Result<(), String> {
    main_only(&window)?;
    settings.validate()?;
    let mut data = state.data.lock().unwrap();
    settings.x = data.settings.x;
    settings.y = data.settings.y;
    settings.panel_x = data.settings.panel_x;
    settings.panel_y = data.settings.panel_y;
    apply_overlay(&app, &settings, data.editing)?;
    state.db.save_settings(&settings)?;
    if data.settings.translation_provider != settings.translation_provider
        || data.settings.azure_region != settings.azure_region
    {
        data.generation += 1;
        state.retry.fetch_add(1, Ordering::Relaxed);
    }
    data.settings = settings;
    Ok(())
}

fn edit_overlay(app: &tauri::AppHandle, editing: bool) -> Result<(), String> {
    let state = app.state::<Shared>();
    let mut data = state.data.lock().unwrap();
    apply_overlay(app, &data.settings, editing)?;
    data.editing = editing;
    Ok(())
}
#[tauri::command]
fn set_editing(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    editing: bool,
) -> Result<(), String> {
    main_only(&window)?;
    edit_overlay(&app, editing)
}
#[tauri::command]
fn drag_overlay(
    window: tauri::WebviewWindow,
    app: tauri::AppHandle,
    state: State<'_, Shared>,
) -> Result<(), String> {
    if window.label() != "overlay" || !state.data.lock().unwrap().editing {
        return Err("請先開啟浮窗調整模式".into());
    }
    #[cfg(target_os = "macos")]
    return macos::drag(&app);
    #[cfg(not(target_os = "macos"))]
    window.start_dragging().map_err(|e| e.to_string())
}

#[tauri::command]
fn clear_local_data(
    window: tauri::WebviewWindow,
    state: State<'_, Shared>,
    kind: storage::DataKind,
) -> Result<(), String> {
    main_only(&window)?;
    let mut data = state.data.lock().unwrap();
    state.db.clear(kind)?;
    // Invalidate pending work without reloading and immediately recreating the cache.
    data.generation += 1;
    data.loading = false;
    data.translating = false;
    data.translation_message = None;
    Ok(())
}

#[tauri::command]
fn retry(window: tauri::WebviewWindow, state: State<'_, Shared>) -> Result<(), String> {
    main_only(&window)?;
    state.retry.fetch_add(1, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
async fn search_lyrics(
    window: tauri::WebviewWindow,
    state: State<'_, Shared>,
    track_id: String,
) -> Result<Lyrics, String> {
    main_only(&window)?;
    let (track, generation) = {
        let mut data = state.data.lock().unwrap();
        let track = data
            .playback
            .track
            .clone()
            .filter(|track| track.id == track_id)
            .ok_or("歌曲已切換，請重新搜尋")?;
        data.search_results = None;
        (track, data.generation)
    };
    let result = lyrics::search(&state.api, &track).await?;
    finish_lyric_search(&mut state.data.lock().unwrap(), generation, result)
}

fn finish_lyric_search(
    data: &mut Runtime,
    generation: u64,
    result: Lyrics,
) -> Result<Lyrics, String> {
    if data.generation != generation {
        return Err("歌曲或設定已更新，請重新搜尋".into());
    }
    data.search_results = Some(result.clone());
    Ok(result)
}

#[tauri::command]
fn select_lyrics(
    window: tauri::WebviewWindow,
    state: State<'_, Shared>,
    track_id: String,
    candidate_id: u64,
) -> Result<(), String> {
    main_only(&window)?;
    let mut data = state.data.lock().unwrap();
    let track = data
        .playback
        .track
        .as_ref()
        .filter(|track| track.id == track_id)
        .ok_or("歌曲已切換，請重新選擇")?;
    let Some(Lyrics::Candidates { candidates }) =
        data.search_results.as_ref().or(data.lyrics.as_ref())
    else {
        return Err("候選清單已更新，請重新搜尋".into());
    };
    let candidate = candidates
        .iter()
        .find(|candidate| candidate.id == candidate_id)
        .ok_or("找不到此候選版本")?;
    let lyrics = candidate
        .lyrics
        .as_deref()
        .ok_or("候選歌詞已失效，請重新搜尋")?;
    state
        .db
        .select_lyrics(&track.cache_key(), candidate.id, lyrics)?;
    data.generation += 1;
    data.lyrics = None;
    data.search_results = None;
    data.loading = true;
    data.translating = false;
    data.translation_message = None;
    state.retry.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

#[tauri::command]
async fn control_playback(
    window: tauri::WebviewWindow,
    action: playback::PlaybackAction,
) -> Result<(), String> {
    main_only(&window)?;
    playback::control(action).await
}

#[tauri::command]
async fn seek_playback(
    window: tauri::WebviewWindow,
    state: State<'_, Shared>,
    track_id: String,
    position_ms: u64,
) -> Result<(), String> {
    main_only(&window)?;
    let duration = {
        let data = state.data.lock().unwrap();
        data.playback
            .track
            .as_ref()
            .filter(|track| track.id == track_id)
            .ok_or("歌曲已切換，請重試")?
            .duration_ms
    };
    playback::seek(position_ms, duration).await
}

#[tauri::command]
async fn key_status(
    window: tauri::WebviewWindow,
    provider: TranslationProvider,
) -> Result<bool, String> {
    main_only(&window)?;
    tauri::async_runtime::spawn_blocking(move || {
        storage::read_key(provider).map(|key| key.is_some())
    })
    .await
    .map_err(|_| "無法查詢系統憑證".to_string())?
}
#[tauri::command]
async fn save_api_key(
    window: tauri::WebviewWindow,
    state: State<'_, Shared>,
    key: String,
    provider: TranslationProvider,
) -> Result<(), String> {
    main_only(&window)?;
    let key = key.trim().to_string();
    if key.len() > 256
        || !key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    {
        return Err("API key 格式無效".into());
    }
    tauri::async_runtime::spawn_blocking(move || storage::write_key(provider, &key))
        .await
        .map_err(|_| "無法更新系統憑證".to_string())??;
    state.retry.fetch_add(1, Ordering::Relaxed);
    Ok(())
}
#[tauri::command]
async fn open_automation_settings(window: tauri::WebviewWindow) -> Result<(), String> {
    main_only(&window)?;
    #[cfg(target_os = "macos")]
    {
        let status = tokio::process::Command::new("/usr/bin/open")
            .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Automation")
            .status()
            .await
            .map_err(|_| "無法開啟系統設定")?;
        if !status.success() {
            return Err("無法開啟系統設定".into());
        }
    }
    Ok(())
}

fn update_current(state: &Shared, generation: u64, update: impl FnOnce(&mut Runtime)) {
    let mut data = state.data.lock().unwrap();
    if data.generation == generation {
        update(&mut data);
    }
}

async fn load_track(state: Shared, track: Track, generation: u64) {
    let lyric_key = format!("lrclib:v1:{}", track.cache_key());
    let cached = state
        .db
        .selected_lyrics(&track.cache_key())
        .and_then(|selected| match selected {
            Some(lyrics) => Ok(Some(lyrics)),
            None => state.db.get(&lyric_key),
        });
    let result = match cached {
        Ok(Some(cached)) => Ok(cached),
        Ok(None) => lyrics::fetch(&state.api, &track).await,
        Err(error) => Err(format!("無法讀取歌詞快取：{error}")),
    };
    let result = match result {
        Ok(lyrics) => lyrics,
        Err(message) => {
            update_current(&state, generation, |data| {
                data.lyrics = Some(Lyrics::Error { message });
                data.loading = false;
            });
            return;
        }
    };
    let cache_warning = {
        let data = state.data.lock().unwrap();
        if data.generation != generation {
            return;
        }
        state
            .db
            .put(&lyric_key, &result)
            .err()
            .map(|_| "無法寫入歌詞快取".to_string())
    };
    update_current(&state, generation, |data| {
        data.lyrics = Some(result.clone());
        data.loading = false;
        data.translation_message = cache_warning;
    });
    if !matches!(result, Lyrics::Synced { .. }) {
        return;
    }
    let settings = {
        let data = state.data.lock().unwrap();
        if data.generation != generation {
            return;
        }
        data.settings.clone()
    };
    let provider = settings.translation_provider;
    let translation_key = translation::cache_key(&track, &result, provider);
    match state.db.get(&translation_key) {
        Ok(Some(cached)) => {
            update_current(&state, generation, |data| data.lyrics = Some(cached));
            return;
        }
        Err(_) => {
            update_current(&state, generation, |data| {
                data.translation_message =
                    Some("無法讀取翻譯快取；為避免重複費用，請先重新啟動 App".into())
            });
            return;
        }
        _ => {}
    }
    let key = tauri::async_runtime::spawn_blocking(move || storage::read_key(provider)).await;
    let key = match key {
        Ok(Ok(Some(key))) => key,
        other => {
            let message = match other {
                Ok(Err(message)) => message,
                Ok(Ok(None)) => "尚未設定所選服務的 API key，先顯示原文".into(),
                _ => "無法讀取系統憑證".into(),
            };
            update_current(&state, generation, |data| {
                data.translation_message = Some(message)
            });
            return;
        }
    };
    {
        let mut data = state.data.lock().unwrap();
        if data.generation != generation {
            return;
        }
        data.translating = true;
    }
    let translated =
        translation::translate(&state.api, &result, &key, provider, &settings.azure_region).await;
    match translated {
        Ok(value) => {
            let warning = {
                let data = state.data.lock().unwrap();
                if data.generation != generation {
                    return;
                }
                state
                    .db
                    .put(&translation_key, &value)
                    .err()
                    .map(|_| "翻譯成功但快取寫入失敗，下次重播可能再次計費".to_string())
            };
            update_current(&state, generation, |data| {
                data.lyrics = Some(value);
                data.translating = false;
                data.translation_message = warning;
            });
        }
        Err(error) => update_current(&state, generation, |data| {
            data.translating = false;
            data.translation_message = Some(format!("翻譯暫時無法使用：{error}"));
        }),
    }
}

async fn coordinate(state: Shared) {
    let clock = Instant::now();
    let provider = playback::Spotify;
    let mut task: Option<tauri::async_runtime::JoinHandle<()>> = None;
    let mut last_retry = 0;
    let mut interval = tokio::time::interval(Duration::from_secs(1));
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        interval.tick().await;
        let mut playback = match provider.get_state().await {
            Ok(value) => value,
            Err(message) => Playback {
                message: Some(message),
                ..Playback::default()
            },
        };
        playback.observed_at_monotonic_ms = clock.elapsed().as_millis() as u64;
        let revision = state.retry.load(Ordering::Relaxed);
        let next = {
            let mut data = state.data.lock().unwrap();
            let changed = data.playback.track != playback.track || revision != last_retry;
            data.playback = playback;
            data.observed = Instant::now();
            if changed {
                data.generation += 1;
                data.lyrics = None;
                data.search_results = None;
                data.loading = data.playback.track.is_some();
                data.translating = false;
                data.translation_message = None;
                Some((data.playback.track.clone(), data.generation))
            } else {
                None
            }
        };
        last_retry = revision;
        if let Some((track, generation)) = next {
            if let Some(old) = task.take() {
                old.abort();
            }
            if let Some(track) = track {
                task = Some(tauri::async_runtime::spawn(load_track(
                    state.clone(),
                    track,
                    generation,
                )));
            }
        }
    }
}

fn show_settings(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_snapshot,
            clear_local_data,
            save_settings,
            set_editing,
            drag_overlay,
            retry,
            search_lyrics,
            select_lyrics,
            control_playback,
            seek_playback,
            key_status,
            save_api_key,
            open_automation_settings
        ])
        .setup(|app| {
            let path = app.path().app_data_dir()?;
            std::fs::create_dir_all(&path)?;
            let db = storage::Storage::open(&path.join("lyrics.sqlite3"))?;
            let settings = db.settings()?;
            let shared = Arc::new(AppState {
                db,
                api: lyrics::Api::new()?,
                retry: AtomicU64::new(0),
                data: Mutex::new(Runtime {
                    playback: Playback::default(),
                    observed: Instant::now(),
                    lyrics: None,
                    search_results: None,
                    loading: false,
                    translating: false,
                    translation_message: None,
                    settings: settings.clone(),
                    editing: false,
                    generation: 0,
                }),
            });
            app.manage(shared.clone());
            let overlay = app.get_webview_window("overlay").ok_or("Overlay missing")?;
            let monitors = overlay.available_monitors()?;
            let saved = settings.x.zip(settings.y).filter(|(x, y)| {
                monitors.iter().any(|m| {
                    let p = m.position();
                    let s = m.size();
                    *x >= p.x
                        && *y >= p.y
                        && (*x as i64 + 120) < p.x as i64 + s.width as i64
                        && (*y as i64 + 60) < p.y as i64 + s.height as i64
                })
            });
            if let Some((x, y)) = saved {
                overlay.set_position(tauri::PhysicalPosition::new(x, y))?;
            } else {
                overlay.center()?;
            }
            #[cfg(target_os = "macos")]
            overlay.set_visible_on_all_workspaces(true)?;
            #[cfg(target_os = "macos")]
            macos::configure_overlay(&overlay, &settings)?;
            apply_overlay(app.handle(), &settings, false)?;
            #[cfg(not(target_os = "macos"))]
            {
                let moved_state = shared.clone();
                overlay.on_window_event(move |event| {
                    if let tauri::WindowEvent::Moved(position) = event {
                        let mut data = moved_state.data.lock().unwrap();
                        data.settings.x = Some(position.x);
                        data.settings.y = Some(position.y);
                        if moved_state.db.save_settings(&data.settings).is_err() {
                            data.translation_message = Some("無法儲存視窗位置".into());
                        }
                    }
                });
            }
            let settings_item =
                MenuItem::with_id(app, "settings", "設定與播放狀態", true, None::<&str>)?;
            let move_item =
                MenuItem::with_id(app, "move", "切換浮動視窗調整模式", true, None::<&str>)?;
            let retry_item = MenuItem::with_id(app, "retry", "重試歌詞與翻譯", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "結束", true, None::<&str>)?;
            let menu =
                Menu::with_items(app, &[&settings_item, &move_item, &retry_item, &quit_item])?;
            let mut pixels = vec![0u8; 18 * 18 * 4];
            for (row, width) in [(4, 14), (8, 10), (12, 12)] {
                for y in row..row + 2 {
                    for x in 2..width + 2 {
                        pixels[(y * 18 + x) * 4 + 3] = 255;
                    }
                }
            }
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(pixels, 18, 18))
                .icon_as_template(true)
                .menu(&menu)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "settings" => show_settings(app),
                    "move" => {
                        let editing = !app.state::<Shared>().data.lock().unwrap().editing;
                        if edit_overlay(app, editing).is_err() {
                            show_settings(app);
                        }
                    }
                    "retry" => {
                        app.state::<Shared>().retry.fetch_add(1, Ordering::Relaxed);
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;
            tauri::async_runtime::spawn(coordinate(shared));
            Ok(())
        })
        .on_window_event(|window, event| {
            if window.label() == "main" {
                let closing = matches!(event, tauri::WindowEvent::CloseRequested { .. });
                // Windows can give the overlay focus while starting a native drag.
                let lost_focus = cfg!(target_os = "macos")
                    && matches!(event, tauri::WindowEvent::Focused(false));
                if closing || lost_focus {
                    if let Some(state) = window.try_state::<Shared>() {
                        let editing = state.data.lock().unwrap().editing;
                        if editing {
                            if let Err(message) = edit_overlay(window.app_handle(), false) {
                                state.data.lock().unwrap().translation_message =
                                    Some(format!("無法結束調整：{message}"));
                            }
                        }
                    }
                }
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("無法啟動 Spotify Lyrics Translator")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                show_settings(app);
            }
        });
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[test]
    fn searching_preserves_active_lyrics_and_rejects_stale_results() {
        let original = Lyrics::Synced {
            lines: vec![model::Line {
                start_time_ms: 0,
                original: "Original".into(),
                translated: Some("原文".into()),
            }],
        };
        let mut data = Runtime {
            playback: Playback::default(),
            observed: Instant::now(),
            lyrics: Some(original.clone()),
            search_results: None,
            loading: false,
            translating: true,
            translation_message: None,
            settings: Settings::default(),
            editing: false,
            generation: 1,
        };
        let candidates = Lyrics::Candidates { candidates: vec![] };
        assert_eq!(
            finish_lyric_search(&mut data, 1, candidates.clone()).unwrap(),
            candidates
        );
        assert_eq!(data.lyrics, Some(original.clone()));
        assert!(data.translating);
        assert!(!data.loading);
        assert!(finish_lyric_search(&mut data, 0, Lyrics::NotFound).is_err());
        assert_eq!(data.search_results, Some(candidates));
        assert_eq!(data.lyrics, Some(original));
    }
}
