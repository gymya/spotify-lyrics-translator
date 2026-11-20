use crate::model::{Lyrics, Settings, TranslationProvider};
use rusqlite::{params, Connection, OptionalExtension};
use std::{path::Path, sync::Mutex};

#[derive(Clone, Copy, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Original,
    Translation,
    Matches,
}

pub struct Storage(Mutex<Connection>);
impl Storage {
    pub fn open(path: &Path) -> Result<Self, String> {
        let db = Connection::open(path).map_err(|e| e.to_string())?;
        db.execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE IF NOT EXISTS cache (key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE IF NOT EXISTS lyric_matches (track_key TEXT PRIMARY KEY, lrclib_id INTEGER NOT NULL, lyrics TEXT NOT NULL); CREATE TABLE IF NOT EXISTS settings (id INTEGER PRIMARY KEY CHECK(id=1), value TEXT NOT NULL);").map_err(|e| e.to_string())?;
        Ok(Self(Mutex::new(db)))
    }
    pub fn clear(&self, kind: DataKind) -> Result<(), String> {
        let mut db = self.0.lock().unwrap();
        let tx = db.transaction().map_err(|e| e.to_string())?;
        let sql = match kind {
            DataKind::Original => "DELETE FROM cache WHERE key LIKE 'lrclib:%'",
            DataKind::Translation => "DELETE FROM cache WHERE key LIKE 'google:%' OR key LIKE 'azure:%'",
            DataKind::Matches => "DELETE FROM cache WHERE key IN (SELECT 'lrclib:v1:' || track_key FROM lyric_matches)",
        };
        tx.execute(sql, []).map_err(|e| e.to_string())?;
        if matches!(kind, DataKind::Matches) {
            tx.execute("DELETE FROM lyric_matches", [])
                .map_err(|e| e.to_string())?;
        }
        tx.commit().map_err(|e| e.to_string())
    }
    pub fn get(&self, key: &str) -> Result<Option<Lyrics>, String> {
        let value: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row("SELECT value FROM cache WHERE key=?1", [key], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        value
            .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn put(&self, key: &str, value: &Lyrics) -> Result<(), String> {
        if matches!(
            value,
            Lyrics::Error { .. } | Lyrics::NotFound | Lyrics::Candidates { .. }
        ) {
            return Ok(());
        }
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO cache(key,value) VALUES(?1,?2)",
                params![
                    key,
                    serde_json::to_string(value).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn selected_lyrics(&self, track_key: &str) -> Result<Option<Lyrics>, String> {
        let value: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT lyrics FROM lyric_matches WHERE track_key=?1",
                [track_key],
                |row| row.get(0),
            )
            .optional()
            .map_err(|e| e.to_string())?;
        value
            .map(|json| serde_json::from_str(&json).map_err(|e| e.to_string()))
            .transpose()
    }
    pub fn select_lyrics(&self, track_key: &str, id: u64, lyrics: &Lyrics) -> Result<(), String> {
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO lyric_matches(track_key,lrclib_id,lyrics) VALUES(?1,?2,?3)",
                params![
                    track_key,
                    id,
                    serde_json::to_string(lyrics).map_err(|e| e.to_string())?
                ],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    pub fn settings(&self) -> Result<Settings, String> {
        let value: Option<String> = self
            .0
            .lock()
            .unwrap()
            .query_row("SELECT value FROM settings WHERE id=1", [], |row| {
                row.get(0)
            })
            .optional()
            .map_err(|e| e.to_string())?;
        let settings: Settings = value
            .map(|v| serde_json::from_str(&v).map_err(|e| e.to_string()))
            .transpose()?
            .unwrap_or_default();
        settings.validate()?;
        Ok(settings)
    }
    pub fn save_settings(&self, settings: &Settings) -> Result<(), String> {
        settings.validate()?;
        self.0
            .lock()
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO settings(id,value) VALUES(1,?1)",
                [serde_json::to_string(settings).map_err(|e| e.to_string())?],
            )
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(target_os = "macos")]
pub fn read_key(provider: TranslationProvider) -> Result<Option<String>, String> {
    match security_framework::passwords::get_generic_password(
        "app.spotify-lyrics-translator.desktop",
        provider.account(),
    ) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| "Keychain 憑證格式無效".into()),
        Err(e) if e.code() == -25300 => Ok(None),
        Err(_) => Err("無法讀取 Keychain，請允許存取鑰匙圈".into()),
    }
}
#[cfg(target_os = "macos")]
pub fn write_key(provider: TranslationProvider, key: &str) -> Result<(), String> {
    use security_framework::passwords::{delete_generic_password, set_generic_password};
    let result = if key.is_empty() {
        match delete_generic_password("app.spotify-lyrics-translator.desktop", provider.account()) {
            Err(e) if e.code() == -25300 => Ok(()),
            other => other,
        }
    } else {
        set_generic_password(
            "app.spotify-lyrics-translator.desktop",
            provider.account(),
            key.as_bytes(),
        )
    };
    result.map_err(|_| "無法更新 Keychain".into())
}
#[cfg(not(target_os = "macos"))]
pub fn read_key(provider: TranslationProvider) -> Result<Option<String>, String> {
    Err("目前只支援 macOS Keychain".into())
}
#[cfg(not(target_os = "macos"))]
pub fn write_key(_provider: TranslationProvider, _: &str) -> Result<(), String> {
    Err("目前只支援 macOS Keychain".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clearing_data_preserves_other_categories_and_settings() {
        for kind in [DataKind::Original, DataKind::Translation, DataKind::Matches] {
            let db = Storage::open(Path::new(":memory:")).unwrap();
            for key in [
                "lrclib:v1:selected",
                "lrclib:v1:other",
                "google:v2:song",
                "azure:v3:song",
            ] {
                db.put(key, &Lyrics::Instrumental).unwrap();
            }
            db.select_lyrics("selected", 123, &Lyrics::Instrumental)
                .unwrap();
            db.save_settings(&Settings {
                offset_ms: 123,
                ..Settings::default()
            })
            .unwrap();
            db.clear(kind).unwrap();
            assert_eq!(
                db.get("lrclib:v1:selected").unwrap().is_some(),
                matches!(kind, DataKind::Translation)
            );
            assert_eq!(
                db.get("lrclib:v1:other").unwrap().is_some(),
                !matches!(kind, DataKind::Original)
            );
            for key in ["google:v2:song", "azure:v3:song"] {
                assert_eq!(
                    db.get(key).unwrap().is_some(),
                    !matches!(kind, DataKind::Translation)
                );
            }
            assert_eq!(
                db.selected_lyrics("selected").unwrap().is_some(),
                !matches!(kind, DataKind::Matches)
            );
            assert_eq!(db.settings().unwrap().offset_ms, 123);
            db.clear(kind).unwrap();
        }
    }

    #[test]
    fn selected_version_is_persistent_and_replaceable() {
        let db = Storage::open(Path::new(":memory:")).unwrap();
        assert!(db.selected_lyrics("song").unwrap().is_none());
        db.select_lyrics("song", 123, &Lyrics::Instrumental)
            .unwrap();
        assert_eq!(
            db.selected_lyrics("song").unwrap(),
            Some(Lyrics::Instrumental)
        );
        let changed = Lyrics::Unsynced {
            text: "A different morning".into(),
        };
        db.select_lyrics("song", 124, &changed).unwrap();
        assert_eq!(db.selected_lyrics("song").unwrap(), Some(changed));
        let id: u64 =
            db.0.lock()
                .unwrap()
                .query_row(
                    "SELECT lrclib_id FROM lyric_matches WHERE track_key='song'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
        assert_eq!(id, 124);
        assert!(db.selected_lyrics("other song").unwrap().is_none());
        db.put("choices", &Lyrics::Candidates { candidates: vec![] })
            .unwrap();
        assert!(db.get("choices").unwrap().is_none());
    }

    #[test]
    fn old_settings_default_to_google_and_azure_settings_persist() {
        let old: Settings = serde_json::from_str(r#"{"opacity":0.8}"#).unwrap();
        assert_eq!(old.translation_provider, TranslationProvider::Google);
        assert!(old.azure_region.is_empty());
        let db = Storage::open(Path::new(":memory:")).unwrap();
        let mut settings = Settings {
            translation_provider: TranslationProvider::Azure,
            azure_region: "eastasia".into(),
            ..old
        };
        db.save_settings(&settings).unwrap();
        assert_eq!(
            db.settings().unwrap().translation_provider,
            TranslationProvider::Azure
        );
        assert_eq!(db.settings().unwrap().azure_region, "eastasia");
        assert_ne!(
            TranslationProvider::Google.account(),
            TranslationProvider::Azure.account()
        );
        settings.azure_region = "eastasia\r\nheader".into();
        assert!(db.save_settings(&settings).is_err());
    }

    #[test]
    fn lyric_colors_default_validate_and_persist() {
        let mut settings: Settings = serde_json::from_str("{}").unwrap();
        assert!(settings.subtitle_background);
        settings.subtitle_background = false;
        assert_eq!(settings.original_color, "#e8eee9");
        assert_eq!(settings.translated_color, "#b0edc3");
        settings.original_color = "#FF0000".into();
        settings.translated_color = "#123abc".into();
        let db = Storage::open(Path::new(":memory:")).unwrap();
        db.save_settings(&settings).unwrap();
        assert!(!db.settings().unwrap().subtitle_background);
        assert_eq!(db.settings().unwrap().original_color, "#FF0000");
        assert_eq!(db.settings().unwrap().translated_color, "#123abc");
        for invalid in ["", "red", "#123", "#gggggg", "#12345678", "#１２"] {
            settings.original_color = invalid.into();
            assert!(settings.validate().is_err());
            settings.original_color = "#ffffff".into();
            settings.translated_color = invalid.into();
            assert!(settings.validate().is_err());
            settings.translated_color = "#000000".into();
        }
    }

    #[test]
    fn cache_and_settings_round_trip() {
        let db = Storage::open(Path::new(":memory:")).unwrap();
        db.put("missing", &Lyrics::NotFound).unwrap();
        assert!(db.get("missing").unwrap().is_none());
        db.put("song", &Lyrics::Instrumental).unwrap();
        assert_eq!(db.get("song").unwrap(), Some(Lyrics::Instrumental));
        let settings = Settings {
            offset_ms: -450,
            ..Settings::default()
        };
        db.save_settings(&settings).unwrap();
        assert_eq!(db.settings().unwrap().offset_ms, -450);
        assert!(Settings {
            opacity: f64::NAN,
            ..settings
        }
        .validate()
        .is_err());
    }
}
