# Checklist manual: PointSpeak

Roteiro para o dono validar o que os checks automáticos não alcançam (tela, microfone, atalho global,
clipboard real). Marque cada item em cada plataforma: **Win** (instalador .exe), **X11** e **Wayland**
(Linux). Anote o que falhar com o texto do erro e, se possível, a saída do terminal (`[pointspeak] …`).

Como rodar:
- **Windows:** gere o instalador no Linux com `./scripts/build-windows.sh` e copie
  `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe`.
- **Linux:** `npm run build && cargo build -p pointspeak --release` e rode `target/release/pointspeak`
  num terminal (os logs saem no stderr). No Wayland, instale `wtype` (ou `ydotool` com o `ydotoold` rodando).

As configurações de Microfone, Modelos, Hardware, Idioma e Atalho ficam na janela de Configurações. Confira os
fluxos interativos da seção 7; os passos abaixo também validam o download real e a troca do modelo ativo.

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
| 3.6 | **Silêncio:** Toggle, tocar, falar uma frase e ficar calado | Para sozinho ~2,5 s depois da fala e cola (com o Silero VAD ativo, veja 8.7) | ☐ | ☐ | ☐ |
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

## 7. Configurações

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 7.1 | Configurações → Modelos → baixe um modelo real (ex.: `tiny`) | Progresso em bytes, verificação de integridade e estado `Pronto`; espaço usado atualizado | ☐ | ☐ | ☐ |
| 7.2 | Baixe outro modelo e escolha `Usar este`; depois selecione o anterior | O selo `Ativo` acompanha a troca e o estado do motor/bandeja reflete o modelo escolhido | ☐ | ☐ | ☐ |
| 7.3 | Tente remover o modelo ativo e depois um modelo pronto | A remoção do ativo fica desabilitada com o motivo; o outro pede confirmação antes de apagar | ☐ | ☐ | ☐ |
| 7.4 | Configurações → Hardware; selecione CPU ou um backend disponível e feche/reabra | A seleção fica salva; cartões indisponíveis mostram o motivo; `auto` marca o backend efetivo | ☐ | ☐ | ☐ |
| 7.5 | Configurações → Atalho → `Alterar`; pressione `Ctrl+Shift+Space` | A combinação capturada aparece e passa a acionar o ditado | ☐ | ☐ | ☐ |
| 7.6 | Configurações → Microfone → `Testar microfone`; fale e depois pare | Medidor responde à voz; o teste não grava áudio e encerra ao sair da seção | ☐ | ☐ | ☐ |
| 7.7 | Ative o tema escuro do sistema e abra cada seção de Configurações | Texto, estados de erro/aviso, rádios, barras e botões seguem legíveis | ☐ | ☐ | ☐ |
| 7.8 | Reduza a janela de Configurações para 360 × 480 px e percorra as seções | Navegação, caminho longo da pasta e cartões cabem sem rolagem horizontal | ☐ | ☐ | ☐ |

## 8. Aceleração: ONNX Runtime e hardware

O ONNX Runtime é carregado e roda o Silero VAD da parada por silêncio (ainda não transcreve: o núcleo puro da engine Whisper ONNX já existe, mas a engine no app chega na próxima etapa; a lista de modelos continua só com os `.bin` ggml). Os avisos saem no
terminal como `[pointspeak] ONNX Runtime …`.

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 8.1 | Abra a pasta de instalação (`%LOCALAPPDATA%\PointSpeak` ou a escolhida no instalador) | `onnxruntime.dll` e `DirectML.dll` ao lado do `pointspeak.exe`; licenças em `licenses\` | ☐ | — | — |
| 8.2 | Configurações → Hardware num PC com GPU/NPU DirectX 12 | `NPU (DirectML)` disponível (detalhe com o fabricante, quando o Windows informa); OpenVINO `Nenhum dispositivo Intel compatível`, CUDA `Nenhuma GPU NVIDIA encontrada`, Vulkan `Não incluído nesta versão`, CPU `Sempre disponível` | ☐ | — | — |
| 8.3 | Feche o app, renomeie o `onnxruntime.dll` da pasta de instalação e abra de novo | O app abre normalmente; DirectML, OpenVINO e CUDA com `ONNX Runtime não encontrado`; ditado segue na CPU. Desfaça a troca de nome depois | ☐ | — | — |
| 8.4 | Selecione `NPU (DirectML)` e dite algo | O ditado funciona na CPU e a seção mostra o aviso de fallback para a CPU (esperado até a engine ONNX) | ☐ | — | — |
| 8.5 | Linux sem ONNX Runtime: abra Configurações → Hardware | DirectML `Disponível só no Windows`; OpenVINO e CUDA `ONNX Runtime não encontrado`; Vulkan `Não incluído nesta versão` | — | ☐ | ☐ |
| 8.6 | Linux: `./scripts/fetch-onnxruntime.sh linux` e rode com `ORT_DYLIB_PATH=$PWD/target/onnxruntime/linux-x64/libonnxruntime.so target/release/pointspeak` | OpenVINO `Nenhum dispositivo Intel compatível` e CUDA `Nenhuma GPU NVIDIA encontrada` (o pacote oficial só traz a CPU); sem erro de carregamento no terminal | — | ☐ | ☐ |
| 8.7 | **Silero VAD:** abra o app num terminal e confira a saída | `[pointspeak] Silero VAD carregado de …\resources\silero_vad.onnx` (Linux: com o `ORT_DYLIB_PATH` do 8.6); no Windows o arquivo fica em `resources\` na pasta de instalação e a licença em `licenses\silero_vad-LICENSE.txt` | ☐ | ☐ | ☐ |
| 8.8 | Com o VAD ativo: Toggle, fale uma frase com pausas curtas entre as palavras e fique calado | Não para nas pausas curtas; para ~2,5 s (`silenceTimeoutMs`) depois da última palavra e cola | ☐ | ☐ | ☐ |
| 8.9 | Com o VAD ativo: Toggle num ambiente com ruído contínuo (ventilador, música baixa sem voz) e não fale; pare com o atalho | HUD mostra `Nada foi ouvido` (o ruído não conta como fala); as barras do HUD continuam reagindo ao ruído | ☐ | ☐ | ☐ |
| 8.10 | Sem ONNX Runtime (passo 8.3 no Windows; Linux sem `ORT_DYLIB_PATH`): repita o 3.6 | Para sozinho pelo RMS como antes, sem erro no HUD; no terminal `[pointspeak] VAD: ONNX Runtime indisponível; parada por silêncio pelo RMS` | ☐ | ☐ | ☐ |
| 8.11 | (Dev, opcional) Vocabulário real do Whisper ONNX: baixe `vocab.json` e `added_tokens.json` de `onnx-community/whisper-tiny` e de `onnx-community/whisper-large-v3-turbo` (URLs no `ONNX_CATALOG`) e rode `WHISPER_VOCAB_DIR=<pasta> cargo test -p pointspeak-core real_vocab -- --ignored --nocapture` para cada um | Passa nos dois; imprime `pt=50267` com 99 idiomas (tiny) e 100 (large-v3-turbo) | — | ☐ | — |
