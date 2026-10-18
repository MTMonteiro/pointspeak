# Comandos da janela de Configurações: armadilhas de teste e de thread

tipo: armadilha
origem: task #781 · claude · 2026-10-18

- **Fake do executor:** os testes de `src-tauri/src/dictation.rs` conferem a sequência exata das chamadas do
  `Backend`. Chamada nova que não é do fluxo do HUD (ex.: `engine_changed`) vai num canal à parte no `Fake`, senão
  quebra todos os `expect` existentes.
- **Atalho testável:** o `HotkeyBinding` recebe `&impl Registrar` (o `AppHandle` implementa); nos testes use o
  `FakeOs` de `shortcut.rs`. Pausa (bandeja) e suspensão (`suspend_hotkey`, captura na tela) são flags separadas:
  o atalho só fica registrado sem nenhuma das duas.
- **Medidor sem gravar:** `AudioRecorder::level_meter()` usa buffer de 0 s; o pipeline avisa `MaxDuration` logo no
  primeiro bloco e quem usa ignora (só `None` = microfone perdido importa).
- **Callback de áudio não para o próprio stream:** `RecordingHandle::stop` junta as threads do áudio, e o
  `on_auto_stop` roda numa delas. Pare a partir de uma thread nova (veja `MicTest::start`).
- O espelho `AppState.hud` só vira `Recording` depois do `StartRecording` (o `EmitState` vem em seguida): o
  `start_mic_test` confere "ditado em andamento" antes e depois de abrir o microfone.
