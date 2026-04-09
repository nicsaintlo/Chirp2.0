// Local transcription (sherpa-onnx / Parakeet TDT) is not compiled into this
// build. All public functions are preserved so the rest of the codebase
// compiles unchanged; functions that require the model return a clear error.

use std::path::PathBuf;
use tauri::AppHandle;

use crate::settings::models_dir;
use crate::state::SherpaRecognizer;

const LOCAL_NOT_SUPPORTED: &str =
    "Local transcription is not available. Switch to the OpenAI backend in Settings.";

// ── Model metadata stubs ─────────────────────────────────────────────────────

fn model_display_size(_model: &str) -> u64 {
    487_000_000
}

pub fn model_size_bytes(model: &str) -> u64 {
    model_display_size(model)
}

pub fn model_dir(model: &str) -> PathBuf {
    let dir_name = match model {
        "parakeet-tdt-0.6b" => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
        _ => "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    };
    models_dir().join("sherpa").join(dir_name)
}

/// Always false — no local model is bundled in this build.
pub fn model_exists(_model: &str) -> bool {
    false
}

// ── Load / transcribe stubs ──────────────────────────────────────────────────

pub fn load_model(_model: &str, _beam_search: bool) -> Result<SherpaRecognizer, String> {
    Err(LOCAL_NOT_SUPPORTED.to_string())
}

pub fn transcribe(
    _recognizer: &SherpaRecognizer,
    _audio: &[f32],
) -> Result<String, String> {
    Err(LOCAL_NOT_SUPPORTED.to_string())
}

// ── Download stub ────────────────────────────────────────────────────────────

pub async fn download_model(_model: &str, _app_handle: AppHandle) -> Result<(), String> {
    Err(LOCAL_NOT_SUPPORTED.to_string())
}

// ── Audio chunking helpers (no native deps) ──────────────────────────────────

pub fn chunk_audio(
    samples: &[f32],
    sample_rate: u32,
    chunk_secs: f32,
    overlap_secs: f32,
) -> Vec<&[f32]> {
    let chunk_samples = (chunk_secs * sample_rate as f32) as usize;
    let overlap_samples = (overlap_secs * sample_rate as f32) as usize;
    let step = chunk_samples - overlap_samples;

    if samples.len() <= chunk_samples {
        return vec![samples];
    }

    let mut chunks = Vec::new();
    let mut pos = 0;
    while pos < samples.len() {
        let end = (pos + chunk_samples).min(samples.len());
        chunks.push(&samples[pos..end]);
        if end >= samples.len() {
            break;
        }
        pos += step;
    }
    chunks
}

pub fn merge_transcriptions(segments: Vec<String>) -> String {
    let segments: Vec<&str> = segments
        .iter()
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();
    if segments.is_empty() {
        return String::new();
    }

    let mut merged = segments[0].to_string();
    for next in &segments[1..] {
        let prev_words: Vec<&str> = merged.split_whitespace().collect();
        let next_words: Vec<&str> = next.split_whitespace().collect();
        let max_check = prev_words.len().min(next_words.len()).min(8);
        let mut best_overlap = 0;
        for len in 1..=max_check {
            let suffix = &prev_words[prev_words.len() - len..];
            let prefix = &next_words[..len];
            if suffix.iter().zip(prefix.iter()).all(|(a, b)| {
                a.to_lowercase().trim_matches(|c: char| c.is_ascii_punctuation())
                    == b.to_lowercase().trim_matches(|c: char| c.is_ascii_punctuation())
            }) {
                best_overlap = len;
            }
        }
        if best_overlap > 0 {
            let remainder = next_words[best_overlap..].join(" ");
            if !remainder.is_empty() {
                merged.push(' ');
                merged.push_str(&remainder);
            }
        } else {
            merged.push(' ');
            merged.push_str(next);
        }
    }
    merged
}
