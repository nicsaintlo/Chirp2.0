use crate::llm::{datamark, undatamark, system_prompt_for_mode};

/// Encode f32 PCM samples (16 kHz, mono) as a 16-bit WAV byte vector.
/// Writes the RIFF/WAV header manually — no extra dependencies needed.
fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let num_samples = samples.len() as u32;
    let data_size = num_samples * 2; // 16-bit = 2 bytes per sample
    let mut buf = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    buf.extend_from_slice(b"RIFF");
    buf.extend_from_slice(&(36 + data_size).to_le_bytes());
    buf.extend_from_slice(b"WAVE");

    // fmt chunk  (PCM, 1 ch, 16 kHz, 16-bit)
    buf.extend_from_slice(b"fmt ");
    buf.extend_from_slice(&16u32.to_le_bytes());  // chunk size
    buf.extend_from_slice(&1u16.to_le_bytes());   // PCM
    buf.extend_from_slice(&1u16.to_le_bytes());   // channels
    buf.extend_from_slice(&16000u32.to_le_bytes()); // sample rate
    buf.extend_from_slice(&32000u32.to_le_bytes()); // byte rate (16000*1*2)
    buf.extend_from_slice(&2u16.to_le_bytes());   // block align
    buf.extend_from_slice(&16u16.to_le_bytes());  // bits per sample

    // data chunk
    buf.extend_from_slice(b"data");
    buf.extend_from_slice(&data_size.to_le_bytes());
    for &s in samples {
        let pcm = (s * 32767.0).clamp(-32768.0, 32767.0) as i16;
        buf.extend_from_slice(&pcm.to_le_bytes());
    }

    buf
}

/// Transcribe audio using OpenAI Whisper (`whisper-1`).
/// `samples` must be 16 kHz mono f32 PCM.
pub async fn transcribe_audio(api_key: &str, samples: &[f32]) -> Result<String, String> {
    let wav_bytes = encode_wav(samples);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let part = reqwest::multipart::Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("Failed to create multipart part: {e}"))?;

    let form = reqwest::multipart::Form::new()
        .text("model", "whisper-1")
        .text("response_format", "text")
        .part("file", part);

    let resp = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .header("Authorization", format!("Bearer {api_key}"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("OpenAI transcription request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI transcription error {status}: {body}"));
    }

    // response_format=text returns plain text
    let text = resp
        .text()
        .await
        .map_err(|e| format!("Failed to read transcription response: {e}"))?;

    Ok(text.trim().to_string())
}

/// Polish transcribed text using OpenAI `gpt-4o-mini`.
pub async fn cleanup_text(api_key: &str, text: &str, tone_mode: &str) -> Result<String, String> {
    let prompt = system_prompt_for_mode(tone_mode);
    let marked = datamark(text);

    let input_tokens_est = (text.split_whitespace().count() as f64 * 1.3) as usize;
    let max_tokens = (input_tokens_est * 2).clamp(64, 1024);

    let payload = serde_json::json!({
        "model": "gpt-4o-mini",
        "messages": [
            {"role": "system", "content": prompt},
            {"role": "user", "content": format!(
                "Clean up the following speech-to-text transcription. \
                 The text uses ^ as word separators. Remove the ^ markers, \
                 fix grammar, and output only the cleaned text.\n\n\
                 <transcription>\n{}\n</transcription>",
                marked
            )},
        ],
        "temperature": 0.0,
        "max_tokens": max_tokens,
        "response_format": {"type": "json_object"},
    });

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {e}"))?;

    let resp = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {api_key}"))
        .json(&payload)
        .send()
        .await
        .map_err(|e| format!("OpenAI cleanup request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("OpenAI cleanup error {status}: {body}"));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Failed to parse OpenAI cleanup response: {e}"))?;

    let raw = body["choices"][0]["message"]["content"]
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

    // Sanity check: if output is much longer than input, the model likely
    // followed the transcription as an instruction instead of cleaning it.
    let input_words = text.split_whitespace().count();
    let output_words = result.split_whitespace().count();
    if output_words > input_words * 3 / 2 + 10 {
        log::warn!("OpenAI cleanup output too long ({output_words} words vs {input_words}), using original");
        return Ok(text.to_string());
    }

    Ok(result)
}
