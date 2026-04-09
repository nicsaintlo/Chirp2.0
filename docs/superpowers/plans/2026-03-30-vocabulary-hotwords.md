# Vocabulary & Hotwords Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users pre-register words (names, companies, jargon) that Chirp should recognize accurately, using sherpa-onnx's built-in hotwords API to bias the Parakeet model during beam search decoding.

**Architecture:** New `vocabulary.json` persistence file + Vocabulary page in the frontend. When beam search is enabled and vocabulary entries exist, a hotwords file is generated and passed to the sherpa-onnx recognizer config along with the `bpe.vocab` path. The recognizer is rebuilt when beam search or vocabulary changes. Beam search is a separate toggle from vocabulary — it has standalone value for accent handling.

**Tech Stack:** Rust backend (sherpa-onnx 0.1.10 — no upgrade needed), React/TypeScript frontend, Zustand store, Tauri IPC.

**Key Discovery:** sherpa-onnx 0.1.10 already has `hotwords_file: Option<String>`, `hotwords_score: f32`, and `bpe_vocab: Option<String>` on `OfflineRecognizerConfig` (see `offline_asr.rs:415-427`). The Parakeet model directory already contains `bpe.vocab`. No dependency changes required.

---

## File Structure

### New Files
- `src-tauri/src/vocabulary.rs` — Vocabulary persistence (load/save) and hotwords file generation
- `src/components/settings/VocabularyPage.tsx` — Vocabulary management UI

### Modified Files
- `src-tauri/src/state.rs` — Add `VocabularyEntry` struct, `beam_search` to Settings, `vocabulary` to AppState
- `src-tauri/src/transcribe.rs` — Accept beam search + hotwords config in `load_model()`
- `src-tauri/src/commands.rs` — Add `update_vocabulary`/`get_vocabulary` commands, rebuild recognizer on beam search toggle
- `src-tauri/src/lib.rs` — Register new module + commands, load vocabulary at startup
- `src/stores/appStore.ts` — Add vocabulary state/actions, `beamSearch` setting
- `src/lib/constants.ts` — Add `beamSearch: false` to DEFAULT_SETTINGS
- `src/hooks/useSettingsSync.ts` — Add `beamSearch` to SYNCED_KEYS, add vocabulary sync block
- `src/components/settings/Settings.tsx` — Add Vocabulary nav item + page registration
- `src/components/settings/SettingsPage.tsx` — Add Enhanced Recognition toggle in AI & Output section

---

## Task 1: Rust Data Model — VocabularyEntry + Settings

**Files:**
- Modify: `src-tauri/src/state.rs:34-59` (Settings struct), `src-tauri/src/state.rs:93-99` (near DictionaryEntry), `src-tauri/src/state.rs:155-186` (AppState)

- [ ] **Step 1: Add VocabularyEntry struct to state.rs**

After the `SnippetEntry` struct (line 106), add:

```rust
/// Vocabulary entry for hotword biasing during beam search
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VocabularyEntry {
    pub word: String,
    pub boost: f32,
}
```

- [ ] **Step 2: Add `beam_search` to Settings struct**

After `help_improve` (line 58) in the Settings struct, add:

```rust
    #[serde(default)]
    pub beam_search: bool,
```

And in `impl Default for Settings` (line 73), after `help_improve: false,` (line 89), add:

```rust
            beam_search: false,
```

- [ ] **Step 3: Add `vocabulary` field to AppState**

In the `AppState` struct (line 156), after `snippets` (line 159), add:

```rust
    pub vocabulary: Vec<VocabularyEntry>,
```

Update `AppState::new` signature (line 172) to accept vocabulary:

```rust
    pub fn new(settings: Settings, dictionary: Vec<DictionaryEntry>, snippets: Vec<SnippetEntry>, vocabulary: Vec<VocabularyEntry>, history: Vec<TranscriptionEntry>) -> Self {
        Self {
            settings,
            dictionary,
            snippets,
            vocabulary,
            history,
            recording_state: RecordingState::Idle,
            recording_generation: 0,
            hotkey_status: HotkeyStatus::Idle,
            recognizer: None,
            llm_process: None,
            llm_port: None,
        }
    }
```

- [ ] **Step 4: Verify it compiles**

Run: `cd C:/Users/dutch/chirp/src-tauri && cargo check 2>&1 | head -30`

This will fail because `lib.rs` doesn't pass vocabulary yet — that's expected. Confirm the error is about `AppState::new` argument count, not about the new types.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/state.rs
git commit -m "feat: add VocabularyEntry struct, beam_search setting, vocabulary to AppState"
```

---

## Task 2: Vocabulary Persistence — vocabulary.rs

**Files:**
- Create: `src-tauri/src/vocabulary.rs`
- Modify: `src-tauri/src/settings.rs` (add vocabulary_path, load/save functions)

- [ ] **Step 1: Create vocabulary.rs with hotwords file generation**

Create `src-tauri/src/vocabulary.rs`:

```rust
use crate::state::VocabularyEntry;
use std::path::PathBuf;

/// Generate a hotwords file for sherpa-onnx from vocabulary entries.
/// Format: one entry per line, `word :boost_score`
/// Returns the path to the generated file, or None if vocabulary is empty.
pub fn generate_hotwords_file(
    entries: &[VocabularyEntry],
    app_data_dir: &std::path::Path,
) -> Result<Option<PathBuf>, String> {
    if entries.is_empty() {
        return Ok(None);
    }

    let hotwords_path = app_data_dir.join("hotwords.txt");
    let mut content = String::new();

    for entry in entries {
        if entry.word.trim().is_empty() {
            continue;
        }
        // sherpa-onnx format: "word :score" (one per line)
        content.push_str(&format!("{} :{:.1}\n", entry.word.trim(), entry.boost));
    }

    if content.is_empty() {
        return Ok(None);
    }

    std::fs::write(&hotwords_path, &content)
        .map_err(|e| format!("Failed to write hotwords file: {e}"))?;

    log::info!("Generated hotwords file with {} entries at {}", entries.len(), hotwords_path.display());
    Ok(Some(hotwords_path))
}
```

- [ ] **Step 2: Add vocabulary persistence to settings.rs**

In `src-tauri/src/settings.rs`, add a `vocabulary_path()` function near the existing `dictionary_path()` and `snippets_path()` functions. Also add `load_vocabulary()` and `save_vocabulary()` following the exact same pattern as dictionary:

After `snippets_path()`, add:

```rust
fn vocabulary_path() -> PathBuf {
    config_dir().join("vocabulary.json")
}
```

After `load_snippets()`, add:

```rust
pub fn load_vocabulary() -> Vec<crate::state::VocabularyEntry> {
    let path = vocabulary_path();
    match std::fs::read_to_string(&path) {
        Ok(data) => serde_json::from_str(&data).unwrap_or_else(|e| {
            log::warn!("Corrupted vocabulary JSON, resetting: {e}");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}
```

After `save_snippets()`, add:

```rust
pub fn save_vocabulary(entries: &[crate::state::VocabularyEntry]) -> Result<(), String> {
    let path = vocabulary_path();
    let data = serde_json::to_string_pretty(entries)
        .map_err(|e| format!("Failed to serialize vocabulary: {e}"))?;
    std::fs::write(&path, data)
        .map_err(|e| format!("Failed to write vocabulary: {e}"))?;
    Ok(())
}
```

- [ ] **Step 3: Register the module in lib.rs**

In `src-tauri/src/lib.rs`, add after `mod transcribe;` (line 14):

```rust
mod vocabulary;
```

- [ ] **Step 4: Verify it compiles**

Run: `cd C:/Users/dutch/chirp/src-tauri && cargo check 2>&1 | head -30`

Still expect the `AppState::new` argument error — that's fine.

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/vocabulary.rs src-tauri/src/settings.rs src-tauri/src/lib.rs
git commit -m "feat: add vocabulary persistence and hotwords file generation"
```

---

## Task 3: Wire Vocabulary Into Startup + Commands

**Files:**
- Modify: `src-tauri/src/lib.rs:79-124` (startup loading, AppState::new call)
- Modify: `src-tauri/src/commands.rs` (add update_vocabulary, get_vocabulary)

- [ ] **Step 1: Load vocabulary at startup in lib.rs**

In `src-tauri/src/lib.rs`, after `let initial_snippets = settings::load_snippets();` (line 80), add:

```rust
    let initial_vocabulary = settings::load_vocabulary();
```

Update the `AppState::new` call (lines 118-123) to pass vocabulary:

```rust
            Arc::new(tokio::sync::Mutex::new(AppState::new(
                initial_settings,
                initial_dictionary,
                initial_snippets,
                initial_vocabulary,
                initial_history,
            )))
```

- [ ] **Step 2: Add vocabulary commands to commands.rs**

After the `update_dictionary` command (line 126), add:

```rust
#[tauri::command]
pub async fn get_vocabulary(state: State<'_, SharedState>) -> Result<Vec<VocabularyEntry>, String> {
    let s = state.lock().await;
    Ok(s.vocabulary.clone())
}

#[tauri::command]
pub async fn update_vocabulary(
    entries: Vec<VocabularyEntry>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    if entries.len() > 500 {
        return Err("Vocabulary cannot exceed 500 entries".to_string());
    }
    let mut s = state.lock().await;
    s.vocabulary = entries.clone();
    settings::save_vocabulary(&s.vocabulary)?;
    Ok(())
}
```

Add `VocabularyEntry` to the imports at the top of commands.rs. Find the existing `use crate::state::{...}` line and add `VocabularyEntry` to it.

- [ ] **Step 3: Register commands in lib.rs invoke_handler**

In `src-tauri/src/lib.rs`, in the `invoke_handler` block (around line 131), add after `commands::update_dictionary,` (line 134):

```rust
            commands::get_vocabulary,
            commands::update_vocabulary,
```

- [ ] **Step 4: Verify it compiles**

Run: `cd C:/Users/dutch/chirp/src-tauri && cargo check 2>&1 | head -20`

Expected: Should compile cleanly now (or with only warnings).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/lib.rs src-tauri/src/commands.rs
git commit -m "feat: wire vocabulary into startup loading and Tauri commands"
```

---

## Task 4: Dynamic Recognizer — Beam Search + Hotwords

**Files:**
- Modify: `src-tauri/src/transcribe.rs:45-96` (load_model function)
- Modify: `src-tauri/src/commands.rs` (recognizer rebuild on settings change)
- Modify: `src-tauri/src/lib.rs` (pass beam_search at startup)

- [ ] **Step 1: Update load_model to accept beam search + hotwords config**

In `src-tauri/src/transcribe.rs`, change the `load_model` function signature (line 45) to:

```rust
pub fn load_model(model: &str, beam_search: bool, hotwords_file: Option<&str>) -> Result<SherpaRecognizer, String> {
```

Then update the config construction (lines 76-91). Replace the existing config block:

```rust
    // Find the BPE vocab file (required for beam search hotwords)
    let bpe_vocab = dir.join("bpe.vocab");
    let bpe_vocab_path = if bpe_vocab.exists() {
        Some(bpe_vocab.to_string_lossy().into_owned())
    } else {
        None
    };

    let decoding_method = if beam_search {
        "modified_beam_search"
    } else {
        "greedy_search"
    };

    log::info!(
        "Loading Parakeet TDT model from {} with {} threads, decoding={}",
        dir.display(),
        n_threads,
        decoding_method,
    );

    let config = OfflineRecognizerConfig {
        model_config: OfflineModelConfig {
            transducer: OfflineTransducerModelConfig {
                encoder: Some(encoder.to_string_lossy().into_owned()),
                decoder: Some(decoder.to_string_lossy().into_owned()),
                joiner: Some(joiner.to_string_lossy().into_owned()),
            },
            tokens: Some(tokens.to_string_lossy().into_owned()),
            num_threads: n_threads,
            provider: Some("cpu".to_string()),
            debug: false,
            bpe_vocab: bpe_vocab_path,
            ..Default::default()
        },
        decoding_method: Some(decoding_method.to_string()),
        hotwords_file: hotwords_file.map(|s| s.to_string()),
        hotwords_score: if hotwords_file.is_some() { 1.5 } else { 0.0 },
        ..Default::default()
    };
```

Note: `hotwords_score` is a global default — per-entry scores in the hotwords file override this. Setting it to 1.5 as a baseline; per-entry `:3.0` scores in the file take precedence.

- [ ] **Step 2: Update all call sites of load_model**

In `src-tauri/src/lib.rs` (around line 188), update the startup call:

```rust
                    match transcribe::load_model(&model, s.settings.beam_search, None) {
```

Note: At startup we don't generate the hotwords file yet — vocabulary is loaded but we pass `None` for hotwords. We'll generate and rebuild after startup if needed.

In `src-tauri/src/commands.rs`, find the `download_model` command (around line 609) and update:

```rust
    let recognizer = transcribe::load_model(&model, s.settings.beam_search, None)
```

- [ ] **Step 3: Add recognizer rebuild on beam_search toggle in update_settings**

In `src-tauri/src/commands.rs`, find the `update_settings` command. After settings are saved and the hotkey update logic, add a recognizer rebuild when `beam_search` changes. Look for where `update_settings` saves and emits — after that block, add:

```rust
    // Rebuild recognizer if beam_search setting changed
    if partial.get("beamSearch").is_some() || partial.get("beam_search").is_some() {
        let model = s.settings.model.clone();
        let beam_search = s.settings.beam_search;
        let vocabulary = s.vocabulary.clone();
        if transcribe::model_exists(&model) {
            // Generate hotwords file if beam search enabled and vocabulary non-empty
            let hotwords_path = if beam_search && !vocabulary.is_empty() {
                let app_dir = settings::config_dir();
                vocabulary::generate_hotwords_file(&vocabulary, &app_dir)
                    .unwrap_or_else(|e| { log::error!("Failed to generate hotwords: {e}"); None })
            } else {
                None
            };
            let hotwords_str = hotwords_path.as_ref().map(|p| p.to_string_lossy().into_owned());

            match transcribe::load_model(&model, beam_search, hotwords_str.as_deref()) {
                Ok(recognizer) => {
                    s.recognizer = Some(Arc::new(recognizer));
                    log::info!("Recognizer rebuilt (beam_search={beam_search})");
                }
                Err(e) => {
                    log::error!("Failed to rebuild recognizer with beam_search={beam_search}: {e}");
                    // Fallback: try without hotwords
                    if hotwords_str.is_some() {
                        match transcribe::load_model(&model, beam_search, None) {
                            Ok(recognizer) => {
                                s.recognizer = Some(Arc::new(recognizer));
                                log::info!("Recognizer rebuilt without hotwords (fallback)");
                            }
                            Err(e2) => {
                                log::error!("Beam search fallback failed: {e2}, trying greedy");
                                // Final fallback: greedy search
                                if let Ok(recognizer) = transcribe::load_model(&model, false, None) {
                                    s.recognizer = Some(Arc::new(recognizer));
                                    log::info!("Recognizer rebuilt with greedy search (final fallback)");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
```

Add `use crate::vocabulary;` to the imports at the top of commands.rs.

- [ ] **Step 4: Rebuild recognizer on vocabulary update**

In the `update_vocabulary` command (created in Task 3), after saving to disk, add recognizer rebuild logic:

Replace the `update_vocabulary` command with:

```rust
#[tauri::command]
pub async fn update_vocabulary(
    entries: Vec<VocabularyEntry>,
    state: State<'_, SharedState>,
) -> Result<(), String> {
    if entries.len() > 500 {
        return Err("Vocabulary cannot exceed 500 entries".to_string());
    }
    let mut s = state.lock().await;
    s.vocabulary = entries.clone();
    settings::save_vocabulary(&s.vocabulary)?;

    // Rebuild recognizer if beam search is active
    if s.settings.beam_search {
        let model = s.settings.model.clone();
        if transcribe::model_exists(&model) {
            let hotwords_path = if !entries.is_empty() {
                let app_dir = settings::config_dir();
                vocabulary::generate_hotwords_file(&entries, &app_dir)
                    .unwrap_or_else(|e| { log::error!("Failed to generate hotwords: {e}"); None })
            } else {
                None
            };
            let hotwords_str = hotwords_path.as_ref().map(|p| p.to_string_lossy().into_owned());
            match transcribe::load_model(&model, true, hotwords_str.as_deref()) {
                Ok(recognizer) => {
                    s.recognizer = Some(Arc::new(recognizer));
                    log::info!("Recognizer rebuilt after vocabulary update");
                }
                Err(e) => log::error!("Failed to rebuild recognizer after vocabulary update: {e}"),
            }
        }
    }
    Ok(())
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cd C:/Users/dutch/chirp/src-tauri && cargo check 2>&1 | head -20`

Expected: Clean compile.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/transcribe.rs src-tauri/src/commands.rs src-tauri/src/lib.rs
git commit -m "feat: dynamic recognizer rebuild with beam search and hotwords support"
```

---

## Task 5: Frontend — Store + Constants + Sync

**Files:**
- Modify: `src/lib/constants.ts:1-16` (DEFAULT_SETTINGS)
- Modify: `src/stores/appStore.ts` (VocabularyEntry interface, state, actions)
- Modify: `src/hooks/useSettingsSync.ts:9-19` (SYNCED_KEYS), `119-148` (sync block)

- [ ] **Step 1: Add beamSearch to DEFAULT_SETTINGS**

In `src/lib/constants.ts`, after `helpImprove: false,` (line 15), add:

```typescript
  beamSearch: false,
```

- [ ] **Step 2: Add VocabularyEntry and state to appStore.ts**

After the `SnippetEntry` interface (line 15), add:

```typescript
export interface VocabularyEntry {
  word: string
  boost: number
}
```

In the `AppState` interface, after `// Snippets` section (line 58), add:

```typescript
  // Vocabulary
  vocabulary: VocabularyEntry[]
```

After `// AI Cleanup` section (line 50), add:

```typescript
  // Beam Search
  beamSearch: boolean
```

In the actions section, after `removeSnippet` (line 109), add:

```typescript
  addVocabularyEntry: (word: string, boost?: number) => void
  removeVocabularyEntry: (index: number) => void
  updateVocabularyBoost: (index: number, boost: number) => void
  setVocabulary: (vocabulary: VocabularyEntry[]) => void
```

- [ ] **Step 3: Add store implementation**

In the `create<AppState>` call, after `snippets: [],` (line 152), add:

```typescript
  // Vocabulary
  vocabulary: [],
```

After `aiCleanup: DEFAULT_SETTINGS.aiCleanup,` (line 144), add:

```typescript
  // Beam Search
  beamSearch: DEFAULT_SETTINGS.beamSearch,
```

In the actions section, after `removeSnippet` (line 210), add:

```typescript
  addVocabularyEntry: (word, boost = 3.0) =>
    set((state) => ({ vocabulary: [...state.vocabulary, { word, boost }] })),
  removeVocabularyEntry: (index) =>
    set((state) => ({ vocabulary: state.vocabulary.filter((_, i) => i !== index) })),
  updateVocabularyBoost: (index, boost) =>
    set((state) => ({
      vocabulary: state.vocabulary.map((v, i) => (i === index ? { ...v, boost } : v)),
    })),
  setVocabulary: (vocabulary) => set({ vocabulary }),
```

- [ ] **Step 4: Add beamSearch to SYNCED_KEYS and vocabulary sync**

In `src/hooks/useSettingsSync.ts`, add `'beamSearch'` to SYNCED_KEYS (after `'aiCleanup'`, line 13):

```typescript
  'aiCleanup',
  'beamSearch',
```

After the snippets sync block (lines 144-148), add:

```typescript
      // Sync vocabulary changes
      if (state.vocabulary !== prevState.vocabulary) {
        invoke('update_vocabulary', { entries: state.vocabulary }).then(() => {
          useAppStore.getState().setSettingsSaved(true)
        }).catch((e) => console.error('Failed to sync vocabulary:', e))
      }
```

In the initial load section (after the snippets load around line 54-56), add:

```typescript
    // Load vocabulary
    invoke('get_vocabulary').then((entries) => {
      useAppStore.getState().setVocabulary(entries as VocabularyEntry[])
    }).catch((e) => console.error('Failed to load vocabulary:', e))
```

Add `VocabularyEntry` to the import from `../stores/appStore`.

- [ ] **Step 5: Verify frontend compiles**

Run: `cd C:/Users/dutch/chirp && npm run build 2>&1 | tail -20`

Expected: May have type errors if VocabularyPage doesn't exist yet — that's fine since we haven't imported it anywhere.

- [ ] **Step 6: Commit**

```bash
git add src/lib/constants.ts src/stores/appStore.ts src/hooks/useSettingsSync.ts
git commit -m "feat: add vocabulary state, beam search setting, and sync to frontend store"
```

---

## Task 6: Frontend — Vocabulary Page UI

**Files:**
- Create: `src/components/settings/VocabularyPage.tsx`
- Modify: `src/components/settings/Settings.tsx:1-29` (nav items, pages, imports)

- [ ] **Step 1: Create VocabularyPage.tsx**

Create `src/components/settings/VocabularyPage.tsx`:

```tsx
import { useState } from 'react'
import { trackEvent } from '@aptabase/tauri'
import { ChevronDown } from 'lucide-react'
import { useAppStore } from '../../stores/appStore'
import { Button } from '../shared/Button'
import { Toggle } from '../shared/Toggle'

export function VocabularyPage() {
  const vocabulary = useAppStore((s) => s.vocabulary)
  const addEntry = useAppStore((s) => s.addVocabularyEntry)
  const removeEntry = useAppStore((s) => s.removeVocabularyEntry)
  const updateBoost = useAppStore((s) => s.updateVocabularyBoost)
  const beamSearch = useAppStore((s) => s.beamSearch)
  const updateSettings = useAppStore((s) => s.updateSettings)

  const [newWord, setNewWord] = useState('')
  const [expandedIndex, setExpandedIndex] = useState<number | null>(null)

  const handleAdd = () => {
    const word = newWord.trim()
    if (!word) return
    addEntry(word)
    setNewWord('')
    trackEvent('feature_used', { feature: 'vocabulary_add' })
  }

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (e.key === 'Enter') handleAdd()
  }

  const boostLabel = (boost: number) => {
    if (boost <= 1.5) return 'Low'
    if (boost <= 3.5) return 'Medium'
    return 'High'
  }

  return (
    <div className="flex flex-col gap-5 animate-slide-up">
      <div className="mb-1">
        <h1 className="font-display font-extrabold text-2xl text-[#1a1a1a] tracking-[-0.5px]">
          Vocabulary
        </h1>
        <p className="text-[13px] text-[#aaa] mt-1">
          Words and names you want Chirp to recognize accurately.
        </p>
      </div>

      {/* Beam search toggle */}
      <div className="flex items-center justify-between rounded-card border border-card-border bg-white px-[18px] py-3.5">
        <div>
          <div className="text-[13px] font-medium text-[#1a1a1a]">Enhanced Recognition</div>
          <div className="text-[11px] text-[#aaa] mt-0.5">
            Uses beam search for better accuracy with accents and noise. Slightly slower.
          </div>
        </div>
        <Toggle
          checked={beamSearch}
          onChange={(v) => updateSettings({ beamSearch: v })}
        />
      </div>

      {/* Prerequisite banner */}
      {!beamSearch && vocabulary.length > 0 && (
        <div className="flex items-center justify-between rounded-card border border-chirp-amber-400/30 bg-chirp-amber-400/5 px-[18px] py-3">
          <span className="text-[12px] text-[#999]">
            Enable Enhanced Recognition to use your vocabulary during transcription.
          </span>
          <button
            onClick={() => updateSettings({ beamSearch: true })}
            className="text-[12px] font-medium text-chirp-amber-500 hover:text-chirp-amber-600 transition-colors"
          >
            Enable
          </button>
        </div>
      )}

      {vocabulary.length > 0 ? (
        <div className="overflow-hidden rounded-card border border-card-border">
          {/* Header */}
          <div className="flex bg-[#FAFAF8] px-[18px] py-2.5">
            <span className="flex-1 text-[11px] font-semibold uppercase tracking-[0.5px] text-[#aaa]">
              Word / Phrase
            </span>
            <span className="w-24 text-[11px] font-semibold uppercase tracking-[0.5px] text-[#aaa] text-right mr-10">
              Strength
            </span>
          </div>

          {/* Rows */}
          {vocabulary.map((entry, i) => (
            <div
              key={i}
              className={`border-b border-[#F5F4F0] last:border-b-0 transition-colors hover:bg-[#FAFAF8] group ${
                i % 2 === 0 ? 'bg-white' : 'bg-[#FAFAF8]/50'
              }`}
            >
              <div
                className="flex items-center px-[18px] h-11 animate-slide-up"
                style={{ animationDelay: `${i * 30}ms` }}
              >
                <span className="flex-1 text-[13px] text-[#333]">
                  {entry.word}
                </span>
                <button
                  onClick={() => setExpandedIndex(expandedIndex === i ? null : i)}
                  className="flex items-center gap-1 text-[11px] text-[#aaa] hover:text-[#666] transition-colors mr-2"
                >
                  {boostLabel(entry.boost)}
                  <ChevronDown
                    size={12}
                    className={`transition-transform duration-200 ${expandedIndex === i ? 'rotate-180' : ''}`}
                  />
                </button>
                <button
                  onClick={() => removeEntry(i)}
                  className="flex h-8 w-10 items-center justify-center text-[#ccc] hover:text-chirp-error transition-colors duration-150 opacity-0 group-hover:opacity-100"
                >
                  ✕
                </button>
              </div>

              {/* Expanded boost slider */}
              {expandedIndex === i && (
                <div className="px-[18px] pb-3 pt-1 flex items-center gap-3 animate-slide-up">
                  <span className="text-[11px] text-[#aaa] w-8">Low</span>
                  <input
                    type="range"
                    min="1.0"
                    max="5.0"
                    step="0.5"
                    value={entry.boost}
                    onChange={(e) => updateBoost(i, parseFloat(e.target.value))}
                    className="flex-1 accent-[#1a1a1a] h-1"
                  />
                  <span className="text-[11px] text-[#aaa] w-8 text-right">High</span>
                  <span className="text-[11px] text-[#666] font-medium w-8 text-right">{entry.boost.toFixed(1)}</span>
                </div>
              )}
            </div>
          ))}
        </div>
      ) : (
        <div className="flex items-center justify-center rounded-card border border-dashed border-card-border bg-[#FAFAF8] px-6 py-10">
          <p className="text-[13px] text-[#aaa] text-center">
            No entries yet. Add names, companies, and terms Chirp should recognize.
          </p>
        </div>
      )}

      {/* Add row */}
      <div className="flex items-center gap-3">
        <input
          type="text"
          value={newWord}
          onChange={(e) => setNewWord(e.target.value)}
          onKeyDown={handleKeyDown}
          placeholder="Add a word or phrase..."
          className="flex-1 h-10 rounded-lg border border-card-border bg-white px-3 text-[13px] text-[#333] placeholder:text-[#ccc] focus:border-chirp-yellow focus:shadow-[0_0_0_3px_rgba(240,183,35,0.1)] focus:outline-none transition-all duration-150"
        />
        <Button onClick={handleAdd} disabled={!newWord.trim() || vocabulary.length >= 500}>
          Add
        </Button>
      </div>

      {vocabulary.length >= 450 && (
        <p className="text-xs text-chirp-error">
          You're approaching the maximum of 500 entries ({vocabulary.length}/500).
        </p>
      )}
    </div>
  )
}
```

- [ ] **Step 2: Register VocabularyPage in Settings.tsx**

In `src/components/settings/Settings.tsx`:

Add the import after `SettingsPage` import (line 16):

```typescript
import { VocabularyPage } from './VocabularyPage'
```

Add to `NAV_ITEMS` (after snippets, line 21). Use `Languages` icon from lucide-react:

```typescript
  { id: 'vocabulary', label: 'Vocabulary', icon: Languages },
```

Add `Languages` to the lucide-react import (line 6):

```typescript
import { Home, BookOpen, Zap, Languages, Settings as SettingsIcon, Check, Minus, Square, X, Heart } from 'lucide-react'
```

Add to `PAGES` (after snippets, line 28):

```typescript
  vocabulary: VocabularyPage,
```

- [ ] **Step 3: Verify frontend compiles**

Run: `cd C:/Users/dutch/chirp && npm run build 2>&1 | tail -20`

Expected: Clean build. Check that the `Toggle` component import path is correct — look at how SettingsPage imports it.

- [ ] **Step 4: Commit**

```bash
git add src/components/settings/VocabularyPage.tsx src/components/settings/Settings.tsx
git commit -m "feat: add Vocabulary page with beam search toggle and entry management"
```

---

## Task 7: Frontend — Beam Search Toggle in Settings Page

**Files:**
- Modify: `src/components/settings/SettingsPage.tsx:555-562` (after AI & Output section)

- [ ] **Step 1: Add Enhanced Recognition toggle to SettingsPage**

In `src/components/settings/SettingsPage.tsx`, in the AI & Output section. Find the closing of the Tone/AI cleanup conditional block (around line 555-561). Before the `</Card>` closing tag for the AI & Output section, add a new Row.

The exact placement: after the `{!store.aiCleanup && (` hidden div block (lines 557-560), before the `</Card>` (line 561), add:

```tsx
          <Row last>
            <div>
              <div className="text-[13px] font-medium text-[#1a1a1a]">Enhanced Recognition</div>
              <div className="text-[11px] text-[#aaa] mt-0.5">Better accuracy with accents and noise, slightly slower</div>
            </div>
            <Toggle
              checked={store.beamSearch}
              onChange={(v) => store.updateSettings({ beamSearch: v })}
            />
          </Row>
```

Note: If the Tone row currently has `last` prop, remove `last` from it so the new row becomes the last one.

- [ ] **Step 2: Verify frontend compiles**

Run: `cd C:/Users/dutch/chirp && npm run build 2>&1 | tail -20`

Expected: Clean build.

- [ ] **Step 3: Commit**

```bash
git add src/components/settings/SettingsPage.tsx
git commit -m "feat: add Enhanced Recognition toggle to Settings page"
```

---

## Task 8: Manual Verification

No automated tests exist for Chirp. Verification is manual via the dev server.

- [ ] **Step 1: Start the dev server**

Run: `cd C:/Users/dutch/chirp && npx tauri dev`

Kill any stale `node.exe`/`chirp.exe` first if needed.

- [ ] **Step 2: Verify Vocabulary page appears**

- Open Settings window
- Confirm "Vocabulary" nav item appears in sidebar between Snippets and Settings
- Click it — page should load with empty state

- [ ] **Step 3: Test adding vocabulary entries**

- Add "Akilan Lakshamanan" as a vocabulary entry
- Confirm it appears in the list with "Medium" strength label
- Expand the entry — confirm boost slider appears
- Adjust the slider — confirm value changes
- Delete the entry — confirm it's removed

- [ ] **Step 4: Test beam search toggle**

- Toggle "Enhanced Recognition" ON from the Vocabulary page
- Check that the toggle also reflects ON in the Settings page (AI & Output section)
- Toggle OFF from Settings page — confirm it syncs to Vocabulary page
- Check Rust logs for "Recognizer rebuilt (beam_search=true/false)" messages

- [ ] **Step 5: Test hotwords end-to-end**

- Add "Akilan Lakshamanan" to vocabulary
- Enable Enhanced Recognition
- Hold the hotkey and say "Tell Akilan Lakshamanan I'll be late"
- Check if the transcription output correctly includes "Akilan Lakshamanan"
- Compare with Enhanced Recognition OFF — should see the difference

- [ ] **Step 6: Test fallback behavior**

- Check Rust logs to verify recognizer creation succeeds
- If recognizer fails, confirm fallback chain works (beam without hotwords → greedy)

- [ ] **Step 7: Test persistence**

- Add vocabulary entries, close and reopen the app
- Confirm entries persist
- Confirm beam search toggle persists
- Check that `vocabulary.json` exists in the app data directory
