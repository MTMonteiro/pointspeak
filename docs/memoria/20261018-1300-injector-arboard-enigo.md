# Injector: arboard 3.7 + enigo 0.6, armadilhas

tipo: armadilha
origem: task #773 · claude · 2026-10-18

- **X11, clipboard:** o conteúdo só existe enquanto o `arboard::Clipboard` que o pôs está vivo (o processo serve as requisições). Por isso o injector guarda o `Clipboard` numa thread própria (`pointspeak-injector`) e nunca o descarta: soltar logo depois de restaurar apagaria o clipboard restaurado (sem gerenciador de clipboard).
- **enigo 0.6:** o padrão no Linux é `x11rb` (XTest), sem libxdo; mas ele puxa `xkbcommon` 0.9, que linka a `libxkbcommon` do sistema (pacote `libxkbcommon-dev`, já no `setup-linux.sh`). As letras `Key::A..Z` só existem no Windows; no Linux use `Key::Unicode('v')`. `Key::Insert` existe nos dois.
- **windows 0.62:** `OpenProcess(_, false, pid)` recebe `bool`; `QueryFullProcessImageNameW` precisa da feature `Win32_System_Threading`. Processo elevado nega o `OpenProcess`: trate como "não detectado" (Ctrl+V).
- **WM_CLASS** vem como `instância\0classe\0` (ex.: `gnome-terminal-server\0Gnome-terminal\0`): compare as duas partes (`pointspeak_core::injector::parse_wm_class`).
- **Wayland:** sem app em foco detectável e o enigo (X11) só alcança apps do XWayland; `wtype` não funciona no GNOME (sem `virtual-keyboard`), `ydotool` precisa do `ydotoold`.
- Testes: a sequência de backup/restauração é pura (`paste_preserving_clipboard` + trait `ClipboardLike` com fake). O `src-tauri` só testa o que não abre tela (erros, busca no `PATH` com `tempfile`, texto vazio que volta antes de criar o clipboard).
