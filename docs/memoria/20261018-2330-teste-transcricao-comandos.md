# Teste de transcrição: estado da gravação e o que a tela precisa fazer

tipo: armadilha
origem: task #825 · claude · 2026-10-18

- `TranscriptionTest` (`src-tauri/src/transcription_test.rs`) tem fases `Idle → Starting(id) → Recording(id) → Ended(resultado)`. O `Starting` existe porque abrir o microfone roda fora do lock; um ditado que começa nessa hora troca a fase e o `start` fecha o microfone e recusa.
- `RecordingHandle::finish`/`stop` pode rodar **sob** o lock da fase: os callbacks do áudio (`on_level`, `on_auto_stop`) não tomam esse lock (o `on_auto_stop` só cria uma thread). Não acrescente lock neles.
- A parada sozinha (30 s, microfone perdido, ditado) **não** devolve nada: emite `settings://transcription-test-ended` e guarda o resultado; a tela precisa chamar `stop_transcription_test` em seguida para pegar o texto ou o erro.
- O atalho global não é suspenso pelo backend: a tela chama `suspend_hotkey` em volta do teste (padrão da captura do atalho). O ditado pela bandeja ainda interrompe o teste (`stop_for_dictation`).
- Backend/dispositivo do resultado vêm do `transcriber::runtime(app, plan.key())` lido logo depois de transcrever (a engine que rodou de fato, com fallback).
