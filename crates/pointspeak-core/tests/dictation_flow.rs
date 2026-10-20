use std::f32::consts::TAU;
use std::time::{Duration, Instant};

use pointspeak_core::audio::{CapturePipeline, CaptureSettings, StopReason, TARGET_SAMPLE_RATE};
use pointspeak_core::config::{AppConfig, Backend, Language, ModelId};
use pointspeak_core::dictation::{Dictation, Effect, Input, ModelStatus};
use pointspeak_core::events::{ErrorKind, StateChangedPayload};
use pointspeak_core::hotkey::{HotkeyMode, KeyAction};
use pointspeak_core::hud::HudState;
use pointspeak_core::hud_layout::DisplayServer;
use pointspeak_core::injector::{
    paste_keys_for, paste_preserving_clipboard, paste_strategy, ClipboardError, ClipboardLike,
    FocusedApp, PasteKeys, PasteMethod, PasteOutcome,
};
use pointspeak_core::transcriber::{
    transcribe, EngineKey, EngineSlot, FakeTranscriber, TranscribeOptions, Transcriber,
};
use pointspeak_core::vad::{FakeClassifier, SilenceMode};

#[derive(Default)]
struct FakeClipboard {
    text: Option<String>,
}

impl ClipboardLike for FakeClipboard {
    fn get_text(&mut self) -> Option<String> {
        self.text.clone()
    }

    fn set_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        self.text = Some(text.into());
        Ok(())
    }

    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.text = None;
        Ok(())
    }
}

fn stereo_voice_then_silence() -> Vec<f32> {
    let rate = 48_000usize;
    let voice_frames = (0..rate).map(|i| {
        let left = 0.5 * (TAU * 440.0 * i as f32 / rate as f32).sin();
        [left, 0.0]
    });
    let silence_frames = std::iter::repeat_n([0.0, 0.0], rate * 3);
    voice_frames.chain(silence_frames).flatten().collect()
}

#[test]
fn recording_flows_through_vad_transcriber_hotswap_and_wayland_paste() {
    let mut pipeline = CapturePipeline::with_classifier(
        CaptureSettings {
            input_rate: 48_000,
            channels: 2,
            silence_timeout_ms: Some(2_500),
            max_recording_secs: 10,
        },
        Some(Box::new(FakeClassifier::new(|window| {
            let rms = (window.iter().map(|sample| sample * sample).sum::<f32>()
                / window.len() as f32)
                .sqrt();
            Ok(if rms > 0.05 { 0.9 } else { 0.0 })
        }))),
    )
    .unwrap();
    let mut levels = Vec::new();
    let stop = pipeline.push(&stereo_voice_then_silence(), |level| levels.push(level));

    assert_eq!(stop, Some(StopReason::Silence));
    assert_eq!(pipeline.silence_mode(), Some(SilenceMode::Vad));
    assert_eq!(pipeline.heard_speech(), Some(true));
    assert!(levels.iter().any(|level| *level > 0.5));

    let (audio, heard_speech) = pipeline.finish_recording();
    assert_eq!(heard_speech, Some(true));
    assert_eq!(audio.len(), TARGET_SAMPLE_RATE as usize * 4);
    assert!(audio.iter().copied().fold(0.0_f32, f32::max) > 0.15);
    assert!(audio.iter().copied().fold(0.0_f32, f32::max) < 0.35);

    let mut dictation = Dictation::new("Alt+Z", HotkeyMode::PushToTalk, ModelStatus::Loaded);
    dictation.start();
    let started = Instant::now();
    assert!(dictation
        .handle(Input::Hotkey(KeyAction::Pressed, started))
        .contains(&Effect::StartRecording));
    assert!(dictation
        .handle(Input::AutoStop(stop.unwrap()))
        .contains(&Effect::Transcribe));
    assert_eq!(dictation.hud_state(), HudState::Transcribing);

    let first_key = EngineKey {
        model: ModelId::Tiny,
        backend: Backend::Cpu,
    };
    let active_key = EngineKey {
        backend: Backend::Openvino,
        ..first_key
    };
    let mut slot = EngineSlot::default();
    slot.ensure(first_key, |_| {
        Ok(Box::new(FakeTranscriber::replying("engine antiga")) as Box<dyn Transcriber>)
    })
    .unwrap();
    slot.ensure(active_key, |_| {
        Ok(
            Box::new(FakeTranscriber::replying("  Olá,  mundo [BLANK_AUDIO] "))
                as Box<dyn Transcriber>,
        )
    })
    .unwrap();
    assert_eq!(slot.loaded_key(), Some(active_key));

    let config = AppConfig {
        language: Language::Auto,
        ..AppConfig::default()
    };
    let options = TranscribeOptions::from_config(&config);
    let text = transcribe(slot.engine().unwrap(), &audio, &options).unwrap();
    assert_eq!(text, "Olá, mundo");
    assert_eq!(
        dictation.handle(Input::TranscriptionFinished(Ok(text.clone()))),
        [Effect::Inject(text.clone())]
    );

    let focused = FocusedApp {
        window_class: Vec::new(),
        process_name: Some(r"C:\Windows\System32\WindowsTerminal.exe".into()),
    };
    let keys = paste_keys_for(&focused);
    assert_eq!(keys, PasteKeys::ShiftInsert);
    assert_eq!(
        paste_strategy(DisplayServer::Wayland),
        [
            PasteMethod::Uinput,
            PasteMethod::Wtype,
            PasteMethod::Ydotool,
            PasteMethod::Enigo
        ]
    );
    let mut clipboard = FakeClipboard {
        text: Some("clipboard anterior".into()),
    };
    let mut tried = Vec::new();
    let outcome = paste_preserving_clipboard(
        &mut clipboard,
        &text,
        || {
            for method in paste_strategy(DisplayServer::Wayland) {
                tried.push(method);
                if method == PasteMethod::Ydotool {
                    break;
                }
            }
            Ok::<_, &'static str>(())
        },
        || {},
    )
    .unwrap();

    assert_eq!(outcome, PasteOutcome::Pasted);
    assert_eq!(
        tried,
        [
            PasteMethod::Uinput,
            PasteMethod::Wtype,
            PasteMethod::Ydotool
        ]
    );
    assert_eq!(clipboard.text.as_deref(), Some("clipboard anterior"));
    assert!(dictation
        .handle(Input::InjectFinished(Ok(())))
        .iter()
        .any(|effect| matches!(effect, Effect::ScheduleDismiss(_))));
    assert_eq!(dictation.hud_state(), HudState::Success);
}

#[test]
fn toggle_stops_on_second_press_and_short_push_to_talk_cancels() {
    let started = Instant::now();
    let mut toggle = Dictation::new("Alt+Z", HotkeyMode::Toggle, ModelStatus::Loaded);
    toggle.start();
    toggle.handle(Input::Hotkey(KeyAction::Pressed, started));
    assert!(toggle
        .handle(Input::Hotkey(
            KeyAction::Released,
            started + Duration::from_millis(20),
        ))
        .is_empty());
    let stopped = toggle.handle(Input::Hotkey(
        KeyAction::Pressed,
        started + Duration::from_millis(100),
    ));
    assert!(stopped.contains(&Effect::StopRecording));
    assert!(stopped.contains(&Effect::Transcribe));
    assert_eq!(toggle.hud_state(), HudState::Transcribing);

    let mut push_to_talk = Dictation::new("Alt+Z", HotkeyMode::PushToTalk, ModelStatus::Loaded);
    push_to_talk.start();
    push_to_talk.handle(Input::Hotkey(KeyAction::Pressed, started));
    let cancelled = push_to_talk.handle(Input::Hotkey(
        KeyAction::Released,
        started + Duration::from_millis(100),
    ));
    assert!(cancelled.contains(&Effect::DiscardRecording));
    assert!(!cancelled.contains(&Effect::Transcribe));
    assert_eq!(push_to_talk.hud_state(), HudState::Idle);
}

#[test]
fn no_speech_skips_inference_and_max_duration_caps_audio_before_transcription() {
    let silent = pointspeak_core::dictation::RecordedAudio {
        samples: vec![0.0; TARGET_SAMPLE_RATE as usize],
        heard_speech: Some(false),
    };
    assert!(silent.is_no_speech());
    let mut skipped_engine = FakeTranscriber::replying("não deve ser chamado");
    let transcription = if silent.is_no_speech() {
        Err(ErrorKind::NoSpeech)
    } else {
        transcribe(
            &mut skipped_engine,
            &silent.samples,
            &TranscribeOptions::default(),
        )
        .map_err(|error| error.kind())
    };
    assert_eq!(skipped_engine.calls, 0);

    let mut dictation = Dictation::new("Alt+Z", HotkeyMode::Toggle, ModelStatus::Loaded);
    dictation.start();
    let started = Instant::now();
    dictation.handle(Input::Hotkey(KeyAction::Pressed, started));
    dictation.handle(Input::AutoStop(StopReason::Silence));
    let error = dictation.handle(Input::TranscriptionFinished(transcription));
    assert!(error.iter().any(|effect| matches!(effect,
        Effect::EmitState(StateChangedPayload { state: HudState::Error, message: Some(message), .. })
            if *message == "Nada foi ouvido")));

    let max_recording_config = AppConfig {
        auto_stop_on_silence: false,
        max_recording_secs: 1,
        ..AppConfig::default()
    };
    let mut max_duration = CapturePipeline::new(CaptureSettings::from_config(
        48_000,
        2,
        &max_recording_config,
    ))
    .unwrap();
    let two_seconds = vec![0.2; 48_000 * 2 * 2];
    assert_eq!(
        max_duration.push(&two_seconds, |_| {}),
        Some(StopReason::MaxDuration)
    );
    let (audio, heard_speech) = max_duration.finish_recording();
    assert_eq!(audio.len(), TARGET_SAMPLE_RATE as usize);
    assert_eq!(heard_speech, None);
    assert!(!silent.samples.is_empty());

    let mut max_duration_dictation =
        Dictation::new("Alt+Z", HotkeyMode::Toggle, ModelStatus::Loaded);
    max_duration_dictation.start();
    max_duration_dictation.handle(Input::Hotkey(KeyAction::Pressed, started));
    assert!(max_duration_dictation
        .handle(Input::AutoStop(StopReason::MaxDuration))
        .contains(&Effect::Transcribe));
    let mut transcriber = FakeTranscriber::replying("duração limitada");
    let result = transcribe(
        &mut transcriber,
        &audio,
        &TranscribeOptions::from_config(&max_recording_config),
    )
    .unwrap();
    assert_eq!(result, "duração limitada");
    assert_eq!(transcriber.calls, 1);
    assert!(max_duration_dictation
        .handle(Input::TranscriptionFinished(Ok(result)))
        .contains(&Effect::Inject("duração limitada".into())));
}
