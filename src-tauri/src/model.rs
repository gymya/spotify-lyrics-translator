use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
}
impl Track {
    pub fn cache_key(&self) -> String {
        hash(&serde_json::to_string(self).expect("track serialization"))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Playback {
    pub track: Option<Track>,
    pub position_ms: u64,
    pub is_playing: bool,
    pub status: String,
    pub observed_at_monotonic_ms: u64,
    pub message: Option<String>,
}
impl Default for Playback {
    fn default() -> Self {
        Self {
            track: None,
            position_ms: 0,
            is_playing: false,
            status: "unavailable".into(),
            observed_at_monotonic_ms: 0,
            message: Some("等待 Spotify 播放音樂…".into()),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Line {
    pub start_time_ms: i64,
    pub original: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub translated: Option<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Candidate {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_ms: u64,
    pub kind: String,
    // Candidate lyrics stay in Rust until the user confirms a version.
    #[serde(skip)]
    pub lyrics: Option<Box<Lyrics>>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Lyrics {
    Synced { lines: Vec<Line> },
    Unsynced { text: String },
    Instrumental,
    NotFound,
    Candidates { candidates: Vec<Candidate> },
    Error { message: String },
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TranslationProvider {
    #[default]
    Google,
    Azure,
}
impl TranslationProvider {
    pub fn account(self) -> &'static str {
        match self {
            Self::Google => "google-translation-v2",
            Self::Azure => "azure-translator-v3",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub translation_provider: TranslationProvider,
    pub azure_region: String,
    pub opacity: f64,
    pub font_size: u32,
    pub original_color: String,
    pub translated_color: String,
    pub subtitle_background: bool,
    pub offset_ms: i64,
    pub click_through: bool,
    pub always_on_top: bool,
    pub overlay_enabled: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub panel_x: Option<f64>,
    pub panel_y: Option<f64>,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            translation_provider: TranslationProvider::Google,
            azure_region: String::new(),
            opacity: 0.92,
            font_size: 26,
            original_color: "#e8eee9".into(),
            translated_color: "#b0edc3".into(),
            subtitle_background: true,
            offset_ms: 0,
            click_through: true,
            always_on_top: true,
            overlay_enabled: true,
            x: None,
            y: None,
            panel_x: None,
            panel_y: None,
        }
    }
}
impl Settings {
    pub fn validate(&self) -> Result<(), String> {
        if [&self.original_color, &self.translated_color]
            .iter()
            .any(|color| {
                color.len() != 7
                    || !color.starts_with('#')
                    || !color.as_bytes()[1..].iter().all(u8::is_ascii_hexdigit)
            })
            || self.azure_region.len() > 64
            || !self
                .azure_region
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
            || self.panel_x.is_some_and(|x| !x.is_finite())
            || self.panel_y.is_some_and(|y| !y.is_finite())
            || !self.opacity.is_finite()
            || !(0.1..=1.0).contains(&self.opacity)
            || !(14..=48).contains(&self.font_size)
            || !(-10_000..=10_000).contains(&self.offset_ms)
        {
            return Err("設定超出允許範圍".into());
        }
        Ok(())
    }
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
    pub playback: Playback,
    pub lyrics: Option<Lyrics>,
    pub line: Option<Line>,
    pub loading: bool,
    pub translating: bool,
    pub translation_message: Option<String>,
    pub settings: Settings,
    pub editing: bool,
}
