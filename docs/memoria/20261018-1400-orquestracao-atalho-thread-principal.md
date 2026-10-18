# Orquestração: atalho global só na thread principal, executor único do ditado

tipo: armadilha
origem: task #774 · claude · 2026-10-18

- **tauri-plugin-global-shortcut 2.4:** `register`/`unregister` rodam na thread principal (`run_on_main_thread`) e
  **bloqueiam** até ela responder. Chamados da thread principal rodam na hora. Um `Mutex` segurado numa thread
  qualquer enquanto registra pode travar com a thread principal esperando o mesmo lock: por isso o
  `HotkeyBinding` (`src-tauri/src/shortcut.rs`) só é usado na thread principal (`setup`, `set_config` síncrono) e o
  executor posta a pausa com `run_on_main_thread` sem esperar. Também não trave nada no handler do atalho (no X11
  ele roda na thread do global-hotkey).
- Comandos Tauri **sem** `async` rodam na thread principal; `set_config` ficou síncrono de propósito.
- `Shortcut` do plugin = `global_hotkey::HotKey` (`Copy + Eq`): compare atalhos por ele, não pelo texto
  (`alt + z` == `Alt+Z`).
- Estado do ditado: só a thread `pointspeak-dictation` muda (máquina pura `pointspeak_core::dictation`).
  `AppState.hud` virou só um espelho (`Mutex<HudState>`) para o `HudController` reemitir a âncora.
- Testes do executor sem hardware: trait `Backend` com fake que responde por threads (veja
  `src-tauri/src/dictation.rs`); o timer de dispensa é o `recv_timeout` do laço, então os testes esperam os 400 ms reais.
- Rodar o app sem tela para ver se o `setup` não quebra: `xvfb-run -a dbus-run-session -- target/debug/pointspeak`
  (com `XDG_CONFIG_HOME`/`XDG_DATA_HOME` temporários). Não há `xdotool` no runner para simular o atalho.
