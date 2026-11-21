use crate::{
    lyrics::{Api, ApiResponse},
    model::{hash, Lyrics, Track, TranslationProvider},
};
use serde::Deserialize;

pub fn cache_key(track: &Track, lyrics: &Lyrics, provider: TranslationProvider) -> String {
    format!(
        "{}:{}:{}",
        match provider {
            TranslationProvider::Google => "google:v2:zh-TW",
            TranslationProvider::Azure => "azure:v3:zh-Hant",
        },
        track.cache_key(),
        hash(&serde_json::to_string(lyrics).expect("lyrics serialization"))
    )
}
#[derive(Deserialize)]
struct Response {
    data: Data,
}
#[derive(Deserialize)]
struct Data {
    translations: Vec<Translated>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Translated {
    translated_text: String,
}

fn api_error(
    provider: TranslationProvider,
    status: u16,
    summary: &str,
    body: Option<&serde_json::Value>,
    key: &str,
) -> String {
    let error = body.and_then(|body| body.get("error"));
    let message = error
        .and_then(|error| error.get("message"))
        .and_then(|value| value.as_str());
    let code = error
        .and_then(|error| error.get("status"))
        .and_then(|value| value.as_str());
    let service = match provider {
        TranslationProvider::Google => "Google Translation",
        TranslationProvider::Azure => "Azure Translator",
    };
    let mut text = format!("{service}（HTTP {status}）：{summary}");
    if let Some(code) = code {
        text.push_str(&format!(" [{code}]"));
    }
    if let Some(message) = message {
        text.push_str(&format!("\n{message}"));
    }
    // Redact before truncation so a partial key cannot leak into the UI.
    if !key.is_empty() {
        text = text.replace(key, "[API key 已隱藏]");
    }
    text.chars().take(2000).collect()
}

// Stay below both the 128-item limit and the Basic API request character limit.
fn batches(texts: &[String]) -> Result<Vec<std::ops::Range<usize>>, String> {
    let mut batches = Vec::new();
    let mut start = 0;
    let mut chars = 0;
    for (i, text) in texts.iter().enumerate() {
        let length = text.chars().count();
        if length > 5000 {
            return Err("單行歌詞過長，暫不翻譯".into());
        }
        if i > start && (i - start >= 128 || chars + length > 5000) {
            batches.push(start..i);
            start = i;
            chars = 0;
        }
        chars += length;
    }
    if start < texts.len() {
        batches.push(start..texts.len());
    }
    Ok(batches)
}

pub async fn translate(
    api: &Api,
    lyrics: &Lyrics,
    key: &str,
    provider: TranslationProvider,
    region: &str,
) -> Result<Lyrics, String> {
    let Lyrics::Synced { lines } = lyrics else {
        return Ok(lyrics.clone());
    };
    let mut result = lines.clone();
    let indices: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| !line.original.is_empty())
        .map(|(i, _)| i)
        .collect();
    let texts: Vec<String> = indices.iter().map(|&i| lines[i].original.clone()).collect();
    for range in batches(&texts)? {
        let request = match provider {
            TranslationProvider::Google => api.client.post("https://translation.googleapis.com/language/translate/v2")
                .header("X-Goog-Api-Key", key)
                .json(&serde_json::json!({ "q": &texts[range.clone()], "target": "zh-TW", "format": "text" })),
            TranslationProvider::Azure => {
                let mut request = api.client.post("https://api.cognitive.microsofttranslator.com/translate")
                    .query(&[("api-version", "3.0"), ("to", "zh-Hant"), ("textType", "plain")])
                    .header("Ocp-Apim-Subscription-Key", key)
                    .json(&texts[range.clone()].iter().map(|text| serde_json::json!({"Text": text})).collect::<Vec<_>>());
                if !region.is_empty() && region != "global" {
                    request = request.header("Ocp-Apim-Subscription-Region", region);
                }
                request
            }
        };
        let body = match api
            .send_json(
                request,
                &api.translation_after,
                "翻譯網路連線失敗",
                "翻譯服務回傳格式無效",
            )
            .await?
        {
            ApiResponse::Success(body) => body,
            ApiResponse::HttpError { response, message } => {
                let status = response.status().as_u16();
                let body = response.json::<serde_json::Value>().await.ok();
                return Err(api_error(provider, status, &message, body.as_ref(), key));
            }
        };
        let translations = parse_translations(provider, body, range.len())?;
        for (index, translated) in range.zip(translations) {
            result[indices[index]].translated = Some(translated);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Ok(Lyrics::Synced { lines: result })
}

fn parse_translations(
    provider: TranslationProvider,
    body: serde_json::Value,
    expected: usize,
) -> Result<Vec<String>, String> {
    let texts = match provider {
        TranslationProvider::Google => {
            let data: Response =
                serde_json::from_value(body).map_err(|_| "翻譯服務回傳格式無效")?;
            data.data
                .translations
                .into_iter()
                .map(|t| html_escape::decode_html_entities(&t.translated_text).into_owned())
                .collect::<Vec<_>>()
        }
        TranslationProvider::Azure => {
            #[derive(Deserialize)]
            struct Item {
                translations: Vec<Text>,
            }
            #[derive(Deserialize)]
            struct Text {
                text: String,
                to: String,
            }
            let items: Vec<Item> =
                serde_json::from_value(body).map_err(|_| "翻譯服務回傳格式無效")?;
            items
                .into_iter()
                .map(|item| {
                    item.translations
                        .into_iter()
                        .find(|t| t.to == "zh-Hant")
                        .map(|t| t.text)
                        .ok_or("翻譯服務缺少繁體中文譯文".to_string())
                })
                .collect::<Result<Vec<_>, _>>()?
        }
    };
    if texts.len() != expected {
        return Err("翻譯行數不符，已保留原文".into());
    }
    Ok(texts)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn azure_response_validation_and_key_redaction() {
        let body =
            serde_json::json!([{"translations":[{"text":"你好 &amp; 世界", "to":"zh-Hant"}]}]);
        assert_eq!(
            parse_translations(TranslationProvider::Azure, body.clone(), 1).unwrap(),
            vec!["你好 &amp; 世界"]
        );
        assert!(parse_translations(TranslationProvider::Azure, body, 2).is_err());
        assert!(parse_translations(
            TranslationProvider::Azure,
            serde_json::json!([{"translations":[]}]),
            1
        )
        .is_err());
        assert!(parse_translations(TranslationProvider::Azure, serde_json::json!({}), 1).is_err());
        let body = serde_json::json!({"error":{"message":"invalid secret"}});
        let error = api_error(
            TranslationProvider::Azure,
            401,
            "驗證失敗",
            Some(&body),
            "secret",
        );
        assert!(error.contains("Azure"));
        assert!(!error.contains("secret"));
    }
    #[test]
    fn api_errors_show_details_without_exposing_key() {
        let body = serde_json::json!({"error": {"message": "API key secret-key is invalid", "status": "INVALID_ARGUMENT"}});
        let message = api_error(
            TranslationProvider::Google,
            400,
            "API 請求失敗",
            Some(&body),
            "secret-key",
        );
        assert!(message.contains("HTTP 400"));
        assert!(message.contains("INVALID_ARGUMENT"));
        assert!(message.contains("is invalid"));
        assert!(!message.contains("secret-key"));
        assert!(api_error(
            TranslationProvider::Google,
            429,
            "請於 60 秒後重試",
            None,
            "key"
        )
        .contains("60 秒"));
        assert!(api_error(
            TranslationProvider::Google,
            502,
            "API 請求失敗",
            Some(&serde_json::json!({})),
            ""
        )
        .contains("HTTP 502"));
    }

    #[test]
    fn batch_limits_and_cache_content_identity() {
        assert_eq!(
            batches(&vec!["hello".into(); 129]).unwrap(),
            vec![0..128, 128..129]
        );
        assert_eq!(
            batches(&["a".repeat(3000), "b".repeat(3000)]).unwrap(),
            vec![0..1, 1..2]
        );
        assert!(batches(&["a".repeat(5001)]).is_err());
        let track = Track {
            id: "id".into(),
            title: "Title".into(),
            artist: "Artist".into(),
            album: "".into(),
            duration_ms: 1,
        };
        assert_ne!(
            cache_key(&track, &Lyrics::Instrumental, TranslationProvider::Google),
            cache_key(&track, &Lyrics::Instrumental, TranslationProvider::Azure)
        );
        assert!(
            cache_key(&track, &Lyrics::Instrumental, TranslationProvider::Google)
                .starts_with("google:v2:zh-TW:")
        );
        assert_ne!(
            cache_key(&track, &Lyrics::Instrumental, TranslationProvider::Google),
            cache_key(
                &track,
                &Lyrics::Unsynced {
                    text: "own fixture".into()
                },
                TranslationProvider::Google
            )
        );
    }
}
