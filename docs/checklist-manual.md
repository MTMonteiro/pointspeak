# Checklist manual: PointSpeak

Roteiro para o dono validar o que os checks automáticos não alcançam (tela, microfone, atalho global,
clipboard real). Marque cada item em cada plataforma: **Win** (instalador .exe), **X11** e **Wayland**
(Linux). Anote o que falhar com o texto do erro e, se possível, a saída do terminal (`[pointspeak] …`).

Como rodar:
- **Windows:** gere o instalador no Linux com `./scripts/build-windows.sh` e copie
  `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe`.
- **Linux:** `npm run build && cargo build -p pointspeak --release` e rode `target/release/pointspeak`
  num terminal (os logs saem no stderr). No Wayland, instale `wtype` (ou `ydotool` com o `ydotoold` rodando).

**Antes da fase Telas** a janela de Configurações ainda é um placeholder. Para os itens que dependem dela:
- Modelo: baixe `ggml-tiny.bin`/`ggml-base.bin` de `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/`
  para a pasta dos modelos (`%APPDATA%\com.pointspeak.app\models` / `~/.local/share/com.pointspeak.app/models`).
- Config: com o app **fechado**, edite `config.json` (`%APPDATA%\com.pointspeak.app\` /
  `~/.config/com.pointspeak.app/`), ex.: `{ "model": "base", "hotkey": "Alt+Z", "hotkeyMode": "toggle" }`, e abra
  o app de novo. A troca de atalho com conflito (6.5) só dá para validar quando a tela de Atalho existir; até lá,
  um atalho em conflito no `config.json` aparece no stderr ao abrir o app.

## 1. Instalação e início

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 1.1 | Instalar o .exe (SmartScreen: "Mais informações" → "Executar assim mesmo") | Instala; WebView2 instalado se faltar | ☐ | — | — |
| 1.2 | Abrir o app | Nenhuma janela abre; ícone na bandeja | ☐ | ☐ | ☐ |
| 1.3 | Passar o mouse no ícone | Tooltip `PointSpeak — Nenhum modelo ativo — abra as Configurações` (sem modelo) | ☐ | ☐ | ☐ |
| 1.4 | Abrir o menu da bandeja | `PointSpeak — …` (desabilitado), `Pausar ditado`, `Configurações…`, `Sair` | ☐ | ☐ | ☐ |
| 1.5 | Clique esquerdo no ícone | Abre as Configurações (no Linux pode só abrir o menu: anote) | ☐ | ☐ | ☐ |

## 2. Modelos

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 2.1 | Configurações → baixar o `tiny` | Progresso até `Pronto` | ☐ | ☐ | ☐ |
| 2.2 | Baixar o `base` e `Usar este` | Bandeja: `Carregando modelo…` e depois `Pronto (Alt+Z)` | ☐ | ☐ | ☐ |
| 2.3 | Fechar e abrir o app com o modelo ativo | Bandeja passa por `Carregando modelo…` e fica `Pronto (Alt+Z)` | ☐ | ☐ | ☐ |

## 3. Ditado

Abra um editor de texto (Bloco de Notas / gedit / Kate) com o cursor numa linha vazia.

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 3.1 | **PTT:** segurar `Alt+Z`, falar "teste de ditado", soltar | HUD `Gravando` com barras mexendo → `Transcrevendo...` → check verde → some; o texto aparece no editor | ☐ | ☐ | ☐ |
| 3.2 | Bandeja durante 3.1 | `Gravando…` → `Transcrevendo…` → `Pronto (Alt+Z)` | ☐ | ☐ | ☐ |
| 3.3 | Toque rápido em `Alt+Z` (< 0,3 s) | HUD aparece e some sem transcrever nem colar | ☐ | ☐ | ☐ |
| 3.4 | Segurar `Alt+Z` 2 s calado | HUD `Nada foi ouvido` por ~2 s; nada colado | ☐ | ☐ | ☐ |
| 3.5 | **Toggle** (Configurações → Atalho → Toggle): tocar `Alt+Z`, falar, tocar de novo | Igual a 3.1 | ☐ | ☐ | ☐ |
| 3.6 | **Silêncio:** Toggle, tocar, falar uma frase e ficar calado | Para sozinho ~2,5 s depois da fala e cola | ☐ | ☐ | ☐ |
| 3.7 | Apertar `Alt+Z` durante `Transcrevendo...` | Ignorado; o ditado em curso termina normalmente | ☐ | ☐ | ☐ |
| 3.8 | Ditar em português e em inglês (Idioma `Automático`) | Os dois saem certos | ☐ | ☐ | ☐ |

## 4. Colagem e clipboard

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 4.1 | Copiar "ANTES" (Ctrl+C), ditar no editor, depois colar com Ctrl+V | O ditado entra no editor; o Ctrl+V seguinte cola "ANTES" (clipboard preservado) | ☐ | ☐ | ☐ |
| 4.2 | Copiar uma imagem (print), ditar, colar num editor de imagem | A imagem volta ao clipboard | ☐ | ☐ | ☐ |
| 4.3 | Ditar num terminal (Windows Terminal / gnome-terminal / konsole) | O texto entra no prompt (Shift+Insert no Linux X11) | ☐ | ☐ | ☐ |
| 4.4 | Ditar num navegador (campo de busca) | O texto entra no campo | ☐ | ☐ | ☐ |
| 4.5 | Wayland sem `wtype`/`ydotool` | HUD `Cole com Ctrl+V` por ~4 s; Ctrl+V cola o ditado | — | — | ☐ |

## 5. HUD

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 5.1 | Durante a gravação, olhar o foco | A janela em foco continua a mesma (título ativo, cursor de texto piscando) | ☐ | ☐ | ☐ |
| 5.2 | Durante a gravação, clicar exatamente onde está a pílula | O clique chega ao app de baixo (click-through) | ☐ | ☐ | ☐ |
| 5.3 | Mover o mouse gravando | A pílula segue o cursor a +16 px (Wayland: fica fixa no centro-inferior) | ☐ | ☐ | ☐ |
| 5.4 | Gravar com o cursor perto da borda direita e da inferior | A pílula vira para o outro lado e não sai da tela | ☐ | ☐ | — |
| 5.5 | Multi-monitor: gravar e levar o cursor ao outro monitor (escalas diferentes, se houver) | A pílula acompanha e mantém o tamanho certo | ☐ | ☐ | — |
| 5.6 | Gravar com o HUD sobre um app em tela cheia | A pílula fica por cima | ☐ | ☐ | ☐ |
| 5.7 | Sem microfone (desplugar/desativar) e ditar | HUD `Microfone indisponível` por ~3 s | ☐ | ☐ | ☐ |
| 5.8 | Desplugar o microfone USB no meio da gravação | HUD `Microfone indisponível`; nada colado | ☐ | ☐ | ☐ |
| 5.9 | Apagar o arquivo do modelo ativo na pasta e ditar | HUD `Modelo não encontrado` por ~3 s | ☐ | ☐ | ☐ |
| 5.10 | Alternar tema claro/escuro e ativar "Reduzir movimento" no sistema; conferir gravação, transcrição, sucesso e erro | Pílula continua legível; barras em três degraus sem transição, três pontos estáticos na transcrição e fade dentro do tempo do backend | ☐ | ☐ | ☐ |

## 6. Bandeja e atalho

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 6.1 | Bandeja → `Pausar ditado`, apertar `Alt+Z` | Nada acontece; tooltip `Pausado`; item vira `Retomar ditado` | ☐ | ☐ | ☐ |
| 6.2 | Pausado, `Alt+Z` chega ao app em foco | O app recebe a tecla (o atalho foi solto) | ☐ | ☐ | ☐ |
| 6.3 | `Retomar ditado` e ditar | Funciona na hora, sem recarregar o modelo | ☐ | ☐ | ☐ |
| 6.4 | Trocar o atalho para `Ctrl+Shift+Space` | O novo grava; `Alt+Z` não grava mais; tooltip `Pronto (Ctrl+Shift+Space)` | ☐ | ☐ | ☐ |
| 6.5 | Trocar para um atalho já usado (ex.: um atalho global de outro app aberto) | Aviso `<atalho> já é usado por outro app ou pelo sistema. Escolha outra combinação.`; o atalho anterior continua valendo | ☐ | ☐ | ☐ |
| 6.6 | Wayland: o atalho global funciona? | Anote: muitos compositores não deixam (limitação conhecida) | — | — | ☐ |
| 6.7 | `Sair` | O app fecha e o ícone some | ☐ | ☐ | ☐ |
