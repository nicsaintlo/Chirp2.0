# Vocabulary & Hotwords Design Spec

**Date:** 2026-03-30
**Status:** Approved

## Problem

Chirp's Parakeet ASR model frequently misrecognizes proper nouns — especially names, companies, and domain jargon that weren't well-represented in training data. Indian names like "Akilan Lakshamanan" are a primary test case, as the majority of Chirp's users are in India with accents the model handles poorly. The current dictionary feature is post-transcription search-and-replace, which can't address this because the model outputs different garbled variations each time.

## Solution

Leverage sherpa-onnx's hotwords API (PR #3077, merged Feb 2026) to bias the Parakeet TDT model toward user-specified vocabulary during beam search decoding. Users pre-register words they want Chirp to recognize via a new Vocabulary page. When beam search is enabled, these words are passed as hotwords to the recognizer, boosting the model's probability of emitting them.

## Prior Art

- **SuperWhisper** — Uses Whisper's `initial_prompt` for vocabulary biasing, but explicitly disabled this for Parakeet models (different architecture). Also has post-processing replacements.
- **Wispr Flow** — Cloud-based server-side biasing + local replacement rules.
- **Dragon NaturallySpeaking** — Per-user acoustic and language model retraining.
- **Sherpa-onnx hotwords** — Aho-Corasick automaton over BPE-tokenized hotwords, boost score distributed across subword tokens during modified beam search. This is the approach we're using.

## Previous Attempt

An earlier attempt at beam search + hotwords failed because hotwords were passed as raw strings instead of being tokenized through the BPE vocabulary. The fix was attempted at the wrong layer repeatedly. This design avoids that by never manually tokenizing — we provide the raw words and the BPE vocab file path, and let sherpa-onnx handle tokenization internally.

---

## New Setting: Beam Search Toggle

**Setting key:** `beam_search` (boolean, default `false`)

**Behavior:**
- `true` → recognizer uses `modified_beam_search` decoding. If vocabulary entries exist, hotwords file + BPE vocab path are supplied.
- `false` → recognizer uses `greedy_search` (current default). Vocabulary entries are stored but inactive.
- Toggling triggers a recognizer rebuild.

**UI placement:** Two locations, same underlying setting:
1. **Settings page** — Toggle labeled "Enhanced Recognition" with subtitle: "Uses beam search for better accuracy with accents and noise. Slightly slower." Placed near the existing AI Cleanup toggle.
2. **Vocabulary page** — Same toggle shown at the top of the page for in-context discovery.

**Rationale for separate toggle:** Beam search has standalone value beyond hotwords — it improves accuracy for accented speech and noisy environments. Users may want beam search without vocabulary, or may want to disable it for speed even with vocabulary entries saved.

---

## New Feature: Vocabulary Page

### Navigation

New sidebar nav item: "Vocabulary" — placed between Dictionary and Snippets (or after Snippets).

### Page Layout

**Header:** "Vocabulary" title, subtitle "Words and names you want Chirp to recognize accurately"

**Beam search prerequisite banner:** Shown when `beam_search` is `false`. Inline banner: "Vocabulary requires Enhanced Recognition" with a button to enable it (toggles `beam_search` setting). When beam search is on, banner is hidden.

**Beam search toggle:** Shown at top of page, mirrors the Settings toggle.

**Add entry form:** Single text input for the word/phrase + "Add" button. Boost score defaults to 3.0 and is not shown in the default add flow.

**Entry list:** Table/card list with:
- Word/phrase (primary display)
- Expand chevron → reveals per-entry boost slider (1.0–5.0 range) labeled "Recognition strength" with low/medium/high markers
- Delete button
- Entry limit: 500 max, warning at 450+

### Data Model

**File:** `vocabulary.json` in app data directory (alongside `dictionary.json`, `snippets.json`)

**Schema:**
```json
[
  { "word": "Akilan Lakshamanan", "boost": 3.0 },
  { "word": "Kubernetes", "boost": 3.0 },
  { "word": "Anthropic", "boost": 2.5 }
]
```

**Zustand store additions:**
- `vocabulary: VocabularyEntry[]` state
- `addVocabularyEntry(word, boost?)` action
- `removeVocabularyEntry(index)` action
- `updateVocabularyEntryBoost(index, boost)` action

---

## Backend Architecture

### Sherpa-onnx Upgrade

- Update `sherpa-onnx` crate to latest version containing PR #3077 (post Feb 2026)
- Verify `bpe.vocab` file exists in the Parakeet model directory. If not bundled with the current model download, include it.

### Recognizer Rebuild Flow

**Current:** Recognizer built once at model load time in `transcribe.rs` with hardcoded `greedy_search`.

**New:** Recognizer is rebuilt when:
- `beam_search` setting changes
- Vocabulary entries change (add/remove/edit boost)
- Model is reloaded

Same `Arc<SherpaRecognizer>` swap pattern — build new recognizer, then atomically replace the Arc'd reference. In-flight transcriptions complete with the old recognizer.

### Hotwords File Generation

When beam search is enabled and vocabulary is non-empty:
1. Write a temp file to app data directory with format:
   ```
   Akilan Lakshamanan :3.0
   Kubernetes :3.0
   Anthropic :2.5
   ```
2. File path passed to `OfflineRecognizerConfig` as `hotwords_file`
3. BPE vocab file path passed as `bpe_vocab`
4. File is regenerated on every recognizer rebuild

When beam search is enabled but vocabulary is empty:
- Use `modified_beam_search` without hotwords (still benefits from multi-candidate decoding)

When beam search is disabled:
- Use `greedy_search`, no hotwords config

### Recognizer Config

```rust
// In transcribe.rs recognizer creation
if beam_search_enabled {
    config.decoding_method = Some("modified_beam_search".to_string());
    if !vocabulary.is_empty() {
        config.hotwords_file = Some(hotwords_file_path);
    }
    config.model_config.bpe_vocab = Some(bpe_vocab_path);
} else {
    config.decoding_method = Some("greedy_search".to_string());
}
```

### Fallback Chain

On recognizer build failure:
1. Beam search + hotwords → fails?
2. Beam search without hotwords → fails?
3. Greedy search (baseline, always works)
4. Surface notification to user explaining the fallback

### New Tauri Commands

- `update_vocabulary(entries: Vec<VocabularyEntry>)` — saves to disk, emits `settings-changed` event, triggers recognizer rebuild if beam search is on
- `get_vocabulary() -> Vec<VocabularyEntry>` — returns current entries

Existing `update_settings` handles `beam_search` toggle but must trigger recognizer rebuild when the value changes.

### Validation

- On recognizer build: check BPE vocab file exists before attempting creation. Surface clear error if missing.
- Hotwords file format is validated (no empty lines, scores are numeric)
- Never manually tokenize hotwords — sherpa-onnx handles BPE tokenization internally

### Cross-Window Sync

Same pattern as dictionary:
1. Frontend calls `invoke('update_vocabulary', { entries })`
2. Rust saves to disk, emits `settings-changed` event
3. Each window's `useSettingsSync` hook updates local Zustand store

---

## Data Flow

### Setup Flow
```
User adds "Akilan Lakshamanan" to Vocabulary
  → vocabulary.json saved to disk
  → hotwords temp file regenerated
  → recognizer rebuilt with modified_beam_search + hotwords + bpe_vocab
  → settings-changed event emitted to all windows
```

### Transcription Flow (with beam search + vocabulary)
```
Hotkey press → cpal audio capture (16kHz mono)
  → sherpa-onnx modified_beam_search (hotwords bias toward vocabulary entries)
  → raw transcript
  → cleanup::cleanup_text()
  → dictionary::apply_dictionary() (unchanged)
  → snippets::apply_snippets() (unchanged)
  → optional llm::cleanup_text() (unchanged)
  → clipboard + Ctrl+V injection
```

### What Changes vs Today
- Recognizer config is dynamic (greedy or beam search) instead of hardcoded greedy
- Hotwords file generated from vocabulary entries and passed to recognizer
- New `vocabulary.json` persistence file
- New Vocabulary page in frontend
- New beam search toggle in Settings (and Vocabulary page)
- Recognizer rebuild triggered by settings/vocabulary changes

### What Doesn't Change
- Audio capture, cleanup, dictionary, snippets, AI cleanup, injection — all untouched
- Overlay window behavior unchanged
- Cross-window sync uses same event pattern
- Onboarding flow unchanged

---

## Files to Create/Modify

### New Files
- `src/components/settings/VocabularyPage.tsx` — Vocabulary page UI
- `src-tauri/src/vocabulary.rs` — Vocabulary persistence and hotwords file generation

### Modified Files
- `src-tauri/Cargo.toml` — Update sherpa-onnx version
- `src-tauri/src/transcribe.rs` — Dynamic recognizer config (beam search, hotwords, bpe_vocab), rebuild function
- `src-tauri/src/commands.rs` — New `update_vocabulary`/`get_vocabulary` commands, recognizer rebuild on beam search toggle
- `src-tauri/src/state.rs` — Add `VocabularyEntry` struct, vocabulary to app state, `beam_search` to settings
- `src-tauri/src/settings.rs` — Vocabulary persistence (load/save `vocabulary.json`), beam_search setting
- `src-tauri/src/lib.rs` — Register new commands, load vocabulary at startup
- `src/stores/appStore.ts` — Vocabulary state and actions, beam_search setting
- `src/components/settings/SettingsPage.tsx` — Add beam search toggle
- `src/components/Sidebar.tsx` (or equivalent nav) — Add Vocabulary nav item
- `src/App.tsx` (or router) — Add Vocabulary route

---

## Out of Scope

- Phonetic fuzzy matching (future enhancement if hotwords alone aren't sufficient)
- Auto-learning from corrections (no feedback loop — Chirp can't see target apps)
- Pronunciation recording (Dragon-style acoustic adaptation)
- Team/shared vocabulary lists
- Bulk import (can add later)
