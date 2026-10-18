# Produto: Pointspeak

## Ideia
# nome do projeto 
PointSpeak
# Project Specification: Ultra-Lightweight Local AI Voice Dictation & Cursor HUD

## 1. Overview & Goal
Build a high-performance, cross-platform (Windows & Linux) desktop background utility for local voice-to-text dictation using **Tauri v2 (Rust backend + TypeScript/React frontend)**. 
When triggered via a global hotkey, the app captures microphone audio, displays a non-intrusive floating animated pill/bubble attached to the user's mouse cursor showing real-time state (`Recording`, `Transcribing`, `Done`), runs local speech-to-text inference (supporting English and Portuguese) using hardware acceleration (NPU/GPU/CPU), and automatically types/pastes the transcribed text into the currently focused application.

## 2. Core Technical Stack
- **Framework:** Tauri v2 (Rust 2021 edition + TypeScript + Vite + Tailwind CSS).
- **Audio Pipeline (Rust):** 
  - `cpal` for low-latency cross-platform audio capture.
  - `rubato` or `samplerate` for real-time in-memory resampling to 16kHz, 16-bit, mono PCM (zero disk I/O).
  - Silero VAD (via `ort` crate) for Voice Activity Detection and real-time audio RMS level calculation (emitted to frontend for waveform animation).
- **Inference Engine & NPU Support (Rust):**
  - Primary/NPU Engine: `ort` (ONNX Runtime Rust wrapper) with dynamic Execution Providers: **DirectML** (Windows NPU/GPU), **OpenVINO** (Intel NPU on Linux/Windows), **CUDA**, and **CPU**.
  - Secondary/GGML Engine: `whisper-rs` (bindings for `whisper.cpp` with Vulkan/CUDA/AVX2 support) for `.bin`/`.gguf` models.
  - Model Manager module capable of downloading, caching, and hot-swapping local models (`whisper-tiny`, `whisper-base`, `whisper-small`, `whisper-large-v3-turbo`) and selecting language (`pt`, `en`, or `auto`).
- **OS Integration & Input Injection (Rust):**
  - Global Hotkeys: `tauri-plugin-global-shortcut` (supporting both Push-to-Talk and Toggle modes).
  - Cursor Tracking: `rdev` or OS-native APIs (`GetCursorPos` on Win32, X11/Wayland equivalents on Linux) running on a lightweight tick loop ONLY while the HUD is visible.
  - Text Injection: `arboard` (Clipboard backup/restore) + `enigo` (simulated keystrokes/paste), with automatic fallback to `wtype`/`ydotool` on Linux Wayland.

## 3. Window & Cursor HUD Architecture
The application consists of two windows managed by Tauri:
1. **Settings & Model Manager Window:** Standard window (hidden by default, opened via System Tray) to configure microphone, select/download models, choose hardware backend (NPU/DirectML, OpenVINO, Vulkan, CUDA, CPU), set language (`PT-BR` / `EN` / `Auto`), and customize hotkeys.
2. **Cursor HUD Overlay Window:**
   - Properties: `transparent: true`, `decorations: false`, `alwaysOnTop: true`, `skipTaskbar: true`, `resizable: false`, `focusable: false`.
   - Click-through: Must call `window.set_ignore_cursor_events(true)` so it never steals window focus or blocks mouse clicks.
   - Positioning: While active, a Rust background thread polls cursor coordinates `(x, y)` at ~60Hz and positions the HUD window at an offset (e.g., `x + 16px`, `y + 16px`).
   - Visual States (Frontend UI):
     - **Idle:** Hidden (`display: none` / window hidden).
     - **Listening/Recording:** Smooth pill-shaped tooltip with a red/accent pulsing dot and a reactive 5-bar audio visualizer driven by RMS volume events from Rust (`hud://audio-level`).
     - **Transcribing:** Morph into a compact loading spinner / shimmer animation with text "Transcrevendo...".
     - **Success:** Quick 400ms green checkmark fade-out before hiding.

## 4. End-to-End Execution Flow
1. **Startup:** App starts minimized to System Tray. Loads selected model into NPU/VRAM (or keeps lazy-loading configurable).
2. **Trigger Start:** User presses Global Hotkey (e.g., `Ctrl+Space` or `Alt+Z`).
3. **Recording State:**
   - Rust starts `cpal` audio stream into a ring buffer.
   - Rust shows the HUD window, sets click-through, starts the cursor-tracking loop, and emits `hud://state-changed` (`recording`) + `hud://audio-level` (every 30ms).
4. **Trigger Stop:** User releases hotkey (Push-to-Talk) or presses again (Toggle), OR Silero VAD detects >2.5s of silence.
5. **Transcribing State:**
   - Audio stream stops. HUD state updates to `transcribing`.
   - Audio buffer (16kHz f32 PCM) is passed directly in-memory to the inference thread (`ort` or `whisper-rs`).
6. **Injection & Cleanup:**
   - Once text is returned, HUD shows `success` and hides; cursor tracking thread sleeps.
   - Rust saves current user clipboard (`arboard`), puts transcribed text into clipboard, triggers `Ctrl+V` (`Shift+Insert` on terminals if detected) via `enigo`, waits 50ms, and restores original clipboard.

## 5. Deliverables & Implementation Phases
Please generate the project structure and implementation following these steps:
1. **`Cargo.toml` & `tauri.conf.json`:** Complete configuration with necessary feature flags for hardware acceleration (`ort`, `whisper-rs`, `cpal`, `enigo`, `arboard`, `tauri-plugin-global-shortcut`) and multi-window setup.
2. **Rust Backend (`src-tauri/src/`):**
   - `audio.rs`: In-memory 16kHz capture + RMS level emitter.
   - `transcriber.rs`: Trait-based inference engine supporting `whisper-rs` and `ort` (with NPU execution provider initialization and CPU fallback).
   - `hud.rs`: Cursor position tracker, click-through enforcement, and window follower logic.
   - `injector.rs`: Cross-platform non-destructive clipboard paste & typing injection (handling X11/Wayland/Win32 quirks).
   - `main.rs` / `lib.rs`: State management, Tray menu, and global shortcut wiring.
3. **Frontend (`src/`):**
   - `HudOverlay.tsx`: Ultra-lightweight animated floating badge reacting to Tauri events (`recording`, `transcribing`, audio levels).
   - `Settings.tsx`: Clean dashboard for model downloading/switching, language toggle (PT/EN), and NPU/GPU device selection.

**Tipo de software:** desktop

## Plano
Resumo e fases do plano da Fábrica: a primeira task de cada mudança de plano atualiza esta seção.

**Resumo:** PointSpeak é um utilitário desktop (Windows/Linux) em Tauri v2 que fica na bandeja do sistema e faz ditado por voz 100% local: um atalho global grava o microfone, um HUD em pílula segue o cursor mostrando gravando/transcrevendo/ok, o Whisper (whisper-rs na CPU ou ONNX Runtime com NPU/DirectML) transcreve em PT/EN e o texto é colado no app em foco, preservando o clipboard. Uma janela de configurações gerencia microfone, modelos, hardware, idioma e atalho. O Windows é entregue como instalador .exe (NSIS) gerado por compilação cruzada no runner Linux.

### Fases
1. **Fundação do projeto e checks** (atual): scaffold Tauri v2 compilando no Linux, workspace Cargo com crate núcleo testável, checks passando e build cruzado do .exe Windows provado com o esqueleto.
2. Design.
3. Núcleo: áudio, estado e Whisper.
4. Integração com o SO (atalho, HUD no cursor, injeção de texto).
5. Telas.
6. Aceleração NPU/GPU (ort).
7. Qualidade e empacotamento.

### Decisões
- As libs de sistema do Tauri/cpal/whisper.cpp são instaladas no runner por `scripts/setup-linux.sh`.
- Wayland não expõe a posição global do cursor: no X11 e no Windows o HUD segue o cursor; no Wayland fica em posição fixa (centro-inferior da tela ativa) e a injeção usa `wtype`/`ydotool`.
- Windows: código atrás de `cfg(windows)`; o instalador `.exe` (NSIS, com bootstrapper do WebView2, sem modelos) sai do runner Linux por `cargo-xwin`. O build Windows não entra nos checks; roda nas etapas que mexem em dependência nativa e no empacotamento final.
