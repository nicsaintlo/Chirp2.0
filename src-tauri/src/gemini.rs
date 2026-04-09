/// Google Gemini API backend — speech-to-text + text cleanup.
///
/// STT:     gemini-2.0-flash-lite  (multimodal, accepts raw audio)
/// Cleanup: gemma-3-1b-it          (text-only, lightest Gemma via API)
///           ↑ update to "gemma-4-…" once Gemma 4 is available on the API
///
/// Both endpoints accept the same auth header:
///   Authorization: Bearer <google_oauth_token>
///   — or —
///   ?key=<google_api_key>   (simpler; get one free at aistudio.google.com)
///
/// This module supports either form: pass the token/key as `auth` and call
/// `auth_param()` to pick the right transport.

use base64::{engine::general_purpose::STANDARD, Engine};
use crate::llm::{datamark, undatamark, system_prompt_for_mode};

const BASE_URL: &str = "https://generativelanguage.googleapis.com/v1beta/models";
const STT_MODEL: &str = "gemini-2.5-flash";
const CLEANUP_MODEL: &str = "gemini-2.5-flash-lite";

// ── WAV encoding (same as openai.rs) ──────────────────────────────────

fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2;
    let mut buf = Vec::with_capacity(44 + data_size as usize);
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVE");
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());
    buf.extend_from_slice(&1u16.to_le_bytes());    // PCM
    buf.extend_from_slice(&1u16.to_le_bytes());    // mono
    buf.extend_from_slice(&16000u32.to_le_bytes());
    buf.extend_from_slice(&32000u32.to_le_bytes());
    buf.extend_from_slice(&2u16.to_le_bytes());
    buf.extend_from_slice(&16u16.to_le_bytes());
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &s in samples {
        let pcm = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        buf.extend_from_slice(&pcm.to_le_bytes());
    }
    buf
}

// ── Auth helper ────────────────────────────────────────────────────────

/// Build a reqwest client with the correct auth for the given token/key.
/// Google API keys go as `?key=` query params; OAuth tokens as Bearer header.
fn is_api_key(auth: &str) -> bool {
    // API keys from AI Studio start with "AIza"
    auth.starts_with("AIza")
}

fn build_url(model: &str, method: &str, auth: &str) -> String {
    if is_api_key(auth) {
        format!("{BASE_URL}/{model}:{method}?key={auth}")
    } else {
        format!("{BASE_URL}/{model}:{method}")
    }
}

fn apply_auth(builder: reqwest::RequestBuilder, auth: &str) -> reqwest::RequestBuilder {
    if is_api_key(auth) {
        builder // key already in URL
    } else {
        builder.bearer_auth(auth)
    }
}

// ── Speech-to-text ─────────────────────────────────────────────────────

/// Transcribe audio using Gemini's multimodal capability.
/// `samples` must be 16 kHz mono f32 PCM.
pub async fn transcribe_audio(auth: &str, samples: &[f32]) -> Result<String, String> {
    let wav_bytes = encode_wav(samples);
    let wav_b64 = STANDARD.encode(&wav_bytes);

    let payload = serde_json::json!({
        "contents": [{
            "parts": [
                {
                    "inline_data": {
                        "mime_type": "audio/wav",
                        "data": wav_b64
                    }
                },
                {
                    "text": "Transcribe the spoken words in this audio. \
                             Return only the transcription — no labels, \
                             no commentary, no punctuation other than what \
                             was clearly spoken."
                }
            ]
        }],
        "generationConfig": {
            "temperature": 0.0
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let url = build_url(STT_MODEL, "generateContent", auth);
    let resp = apply_auth(client.post(&url), auth)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Gemini STT request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini STT error {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse Gemini STT response: {e}"))?;

    let text = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or("")
        .trim()
        .to_string();

    if text.is_empty() {
        return Err("Gemini returned empty transcription".to_string());
    }

    Ok(text)
}

// ── Text cleanup ───────────────────────────────────────────────────────

/// Polish transcribed text using Gemma via the Gemini API.
pub async fn cleanup_text(auth: &str, text: &str, tone_mode: &str) -> Result<String, String> {
    let system_prompt = system_prompt_for_mode(tone_mode);
    let marked = datamark(text);

    let input_tokens_est = (text.split_whitespace().count() as f64 * 1.3) as usize;
    let max_tokens = (input_tokens_est * 2).clamp(64, 1024) as i64;

    // Gemma chat format: system instruction + user turn
    let payload = serde_json::json!({
        "system_instruction": {
            "parts": [{"text": system_prompt}]
        },
        "contents": [{
            "role": "user",
            "parts": [{
                "text": format!(
                    "Clean up the following speech-to-text transcription. \
                     The text uses ^ as word separators. Remove the ^ markers, \
                     fix grammar, and output ONLY a JSON object with a single \
                     key \"cleaned_text\" containing the result.\n\n\
                     <transcription>\n{}\n</transcription>",
                    marked
                )
            }]
        }],
        "generationConfig": {
            "temperature": 0.0,
            "maxOutputTokens": max_tokens,
            "responseMimeType": "application/json"
        }
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("HTTP client: {e}"))?;

    let url = build_url(CLEANUP_MODEL, "generateContent", auth);
    let resp = apply_auth(client.post(&url), auth)
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("Gemini cleanup request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Gemini cleanup error {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Parse Gemini cleanup response: {e}"))?;

    let raw = body["candidates"][0]["content"]["parts"][0]["text"]
        .as_str()
        .unwrap_or(text)
        .trim();

    let result = if let Ok(json) = serde_json::from_str::<serde_json::Value>(raw) {
        json["cleaned_text"]
            .as_str()
            .unwrap_or(text)
            .trim()
            .to_string()
    } else {
        raw.to_string()
    };

    let result = undatamark(&result);

    // Sanity check: reject runaway outputs
    let input_words = text.split_whitespace().count();
    let output_words = result.split_whitespace().count();
    if output_words > input_words * 3 / 2 + 10 {
        log::warn!("Gemini cleanup output too long ({output_words} vs {input_words} words), using original");
        return Ok(text.to_string());
    }

    Ok(result)
}
