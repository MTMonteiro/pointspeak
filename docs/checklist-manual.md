# Checklist manual: PointSpeak 1.0.0

Roteiro de aceite da versão **1.0.0**: o que o dono confere à mão, porque os checks automáticos não alcançam
tela, microfone, atalho global, clipboard real nem hardware (GPU/NPU). Marque cada item em cada plataforma:
**Win** (instalador .exe), **X11** e **Wayland** (Linux, pelo .deb ou AppImage). `—` = não se aplica. Anote o
que falhar com o texto do erro e, se possível, a saída do terminal (`[pointspeak] …`).

## Entrega 1.0.0

Os arquivos, tamanhos e SHA-256 estão em `docs/tecnologia.md` (**Instaladores 1.0.0**). Antes de instalar,
confira o hash do arquivo copiado: `sha256sum <arquivo>` no Linux, `Get-FileHash <arquivo>` no PowerShell.

- **Windows:** `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_1.0.0_x64-setup.exe`
  (gerado por `./scripts/build-windows.sh`).
- **Linux:** `target/release/bundle/deb/PointSpeak_1.0.0_amd64.deb` (`sudo apt install ./PointSpeak_1.0.0_amd64.deb`)
  e `target/release/bundle/appimage/PointSpeak_1.0.0_amd64.AppImage` (`chmod +x` e rodar), gerados por
  `./scripts/build-linux.sh`. Abra `pointspeak` num terminal para ver os logs (stderr).
- **Logs no Windows:** o `pointspeak.exe` de release é um app gráfico (sem console), então rodá-lo direto no
  PowerShell não mostra o stderr. Para os itens que pedem a saída do terminal (8.7, 8.14, 8.16…), feche o app
  pela bandeja e abra com o stderr num arquivo:
  `Start-Process "$env:LOCALAPPDATA\PointSpeak\pointspeak.exe" -RedirectStandardError "$env:TEMP\pointspeak.log"`;
  depois leia com `Get-Content "$env:TEMP\pointspeak.log" -Wait`.
- **Linux (dev, opcional):** `npm run build && cargo build -p pointspeak --release` e rode
  `target/release/pointspeak` num terminal. No Wayland, instale `wtype` (ou `ydotool` com o `ydotoold` rodando).

## Ordem de execução

Faça uma plataforma por vez (sugestão: Windows, depois Linux X11, depois Linux Wayland), nesta ordem:

1. **Instalação e início** (seção 1; no Linux, também 8.6a/8.6b/8.6c do pacote).
2. **Modelos** (seção 2) com o backend `CPU` ou `Automático`: deixe um modelo ggml ativo (`base`).
3. **Ditado, colagem e HUD** (seções 3, 4 e 5).
4. **Bandeja, atalho e Configurações** (seções 6 e 7).
5. **ONNX Runtime e Silero VAD** (seção 8: 8.1 a 8.10). Deixe por último os passos que renomeiam ou
   corrompem arquivos (8.3, 8.4, 8.10) e desfaça cada um antes de seguir.
6. **Windows: DirectML** (seção 8.A, 8.12 a 8.19), num PC com GPU/NPU DirectX 12.
7. **Riscos aceitos** (seção 9): leia e marque se concorda com cada um.
8. Itens `(Dev, opcional)` (8.11, 8.20) só se for desenvolver o projeto.

Aceite da 1.0.0: todos os itens das seções 1 a 7 e 8.1–8.10 passam nas plataformas marcadas, 8.A passa no
Windows com DirectML (ou fica anotado que não há hardware para testar) e a seção 9 está conferida.

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
| 3.9 | **Destino do texto:** dite com Configurações → Saída em `Colar onde está o cursor` e depois em `Copiar para a área de transferência`; no modo copiar, confira que nada é colado, dê Ctrl+V manualmente e teste o Ctrl+V de novo depois | Colar: o texto entra sozinho. Copiar: o editor fica vazio, o Ctrl+V manual cola o texto ditado (e o conteúdo anterior do clipboard foi substituído), o HUD mostra o sucesso; a escolha persiste ao reabrir | ☐ | ☐ | ☐ |

## 4. Colagem e clipboard

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 4.1 | Copiar "ANTES" (Ctrl+C), ditar no editor, depois colar com Ctrl+V | O ditado entra no editor; o Ctrl+V seguinte cola "ANTES" (clipboard preservado) | ☐ | ☐ | ☐ |
| 4.2 | Copiar uma imagem (print), ditar, colar num editor de imagem | A imagem volta ao clipboard | ☐ | ☐ | ☐ |
| 4.3 | Ditar num terminal (Windows Terminal / gnome-terminal / konsole) | O texto entra no prompt (Shift+Insert no Linux X11) | ☐ | ☐ | ☐ |
| 4.4 | Ditar num navegador (campo de busca) | O texto entra no campo | ☐ | ☐ | ☐ |
| 4.5 | Wayland sem `wtype`/`ydotool` | HUD mostra `Falha ao colar` por ~4 s; o clipboard original é restaurado | — | — | ☐ |
| 4.6 | Com o histórico do clipboard ligado (Win+V no Windows; Klipper/KDE ou outro gerenciador no Linux), ditar uma frase e abrir o histórico | A frase ditada não aparece no histórico (nem na sincronização na nuvem do Windows); o texto colado entra normalmente no app | ☐ | ☐ | ☐ |
| 4.7 | **Windows, colagem com clipboard ocupado:** copie um texto qualquer (ex.: "texto antigo"), dite uma frase com o `large-v3-turbo` num navegador e num editor (VS Code/Notepad) | Entra a frase **ditada**, nunca o "texto antigo"; ~400 ms depois o clipboard volta ao "texto antigo" (cole num campo para conferir). Se o clipboard estiver travado por outro app, o HUD mostra `Falha ao colar` (nada é colado) e o clipboard volta ao "texto antigo" | ☐ | — | — |

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
| 6.8 | Windows: Configurações → Atalho → `Alterar` e toque duas vezes rápido em `Alt`; depois, num editor de texto (ex.: Bloco de Notas), toque `Alt` duas vezes e segure o 2º toque enquanto fala | O atalho vira `Alt+Alt` (com a dica de segurar o 2º toque); o ditado grava enquanto o 2º toque fica apertado e cola ao soltar; a barra de menus do editor **não** fica ativada depois; Alt+Tab e outros atalhos com Alt continuam normais. Repita com `Ctrl+Ctrl` | ☐ | — | — |
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

O ONNX Runtime é carregado, roda o Silero VAD da parada por silêncio e, desde a task #794, a transcrição
nos backends DirectML, OpenVINO e CUDA (engine Whisper ONNX; CPU e Vulkan seguem no whisper.cpp). Com um
desses backends a seção Modelos mostra a variante ONNX de cada modelo (pasta `onnx-<id>` com 4 arquivos,
baixada à parte do `.bin`). Os avisos saem no terminal como `[pointspeak] ONNX Runtime …`, e cada carga de
modelo imprime `[pointspeak] transcrição: <engine> em <backend>` (com `(fallback: <motivo>)` quando cai para
a CPU).

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 8.1 | Abra a pasta de instalação (`%LOCALAPPDATA%\PointSpeak` ou a escolhida no instalador) | `onnxruntime.dll` e `DirectML.dll` ao lado do `pointspeak.exe`; licenças em `licenses\` | ☐ | — | — |
| 8.2 | Configurações → Hardware num PC com GPU/NPU DirectX 12 | `NPU (DirectML)` disponível; o detalhe diz se há NPU: `NPU detectada` (num PC com NPU, ex.: Intel Core Ultra, Snapdragon X, Ryzen AI) ou `Sem NPU · GPU`, com o fabricante quando o Windows informa; OpenVINO `Nenhum dispositivo Intel compatível`, CUDA `Nenhuma GPU NVIDIA encontrada`, Vulkan `Não incluído nesta versão`, CPU `Sempre disponível` | ☐ | — | — |
| 8.3 | Feche o app, renomeie o `onnxruntime.dll` da pasta de instalação e abra de novo | O app abre normalmente; DirectML, OpenVINO e CUDA com `ONNX Runtime não encontrado`; ditado segue na CPU. Desfaça a troca de nome depois | ☐ | — | — |
| 8.4 | Selecione `NPU (DirectML)` com o `onnxruntime.dll` renomeado (8.3) e dite algo | Cartão DirectML indisponível; o ditado funciona na CPU (whisper.cpp) | ☐ | — | — |
| 8.5 | Linux sem ONNX Runtime: abra Configurações → Hardware | DirectML `Disponível só no Windows`; OpenVINO e CUDA `ONNX Runtime não encontrado`; Vulkan `Não incluído nesta versão` | — | ☐ | ☐ |
| 8.6 | Linux: `./scripts/fetch-onnxruntime.sh linux` e rode com `ORT_DYLIB_PATH=$PWD/target/onnxruntime/linux-x64/libonnxruntime.so target/release/pointspeak` | OpenVINO `Nenhum dispositivo Intel compatível` e CUDA `Nenhuma GPU NVIDIA encontrada` (o pacote oficial só traz a CPU); sem erro de carregamento no terminal | — | ☐ | ☐ |
| 8.6a | Linux com o .deb instalado (sem `ORT_DYLIB_PATH`): `dpkg -L point-speak \| grep -E 'onnxruntime\|silero'` e abra Configurações → Hardware | Lista `/usr/lib/PointSpeak/libonnxruntime.so`, `/usr/lib/PointSpeak/resources/silero_vad.onnx` e as licenças em `/usr/lib/PointSpeak/licenses/`; o ONNX Runtime CPU é detectado: OpenVINO `Nenhum dispositivo Intel compatível` e CUDA `Nenhuma GPU NVIDIA encontrada` (não `ONNX Runtime não encontrado`), CPU `Sempre disponível`; no terminal nenhum `ONNX Runtime não encontrado` | — | ☐ | ☐ |
| 8.6b | Linux: rode o AppImage (sem `ORT_DYLIB_PATH`) e repita o 8.6a na seção Hardware | Mesmo resultado do 8.6a; `[pointspeak] Silero VAD carregado de /tmp/.mount_…/usr/lib/PointSpeak/resources/silero_vad.onnx` no terminal logo depois de abrir | — | ☐ | ☐ |
| 8.6c | Linux Wayland com o .deb: `apt install` sem `--no-install-recommends` | `wtype` e `ydotool` instalados junto (Recommends) | — | — | ☐ |
| 8.7 | **Silero VAD:** abra o app num terminal e confira a saída | `[pointspeak] Silero VAD carregado de …\resources\silero_vad.onnx` (Linux: com o `ORT_DYLIB_PATH` do 8.6); no Windows o arquivo fica em `resources\` na pasta de instalação e a licença em `licenses\silero_vad-LICENSE.txt` | ☐ | ☐ | ☐ |
| 8.8 | Com o VAD ativo: Toggle, fale uma frase com pausas curtas entre as palavras e fique calado | Não para nas pausas curtas; para ~2,5 s (`silenceTimeoutMs`) depois da última palavra e cola | ☐ | ☐ | ☐ |
| 8.9 | Com o VAD ativo: Toggle num ambiente com ruído contínuo (ventilador, música baixa sem voz) e não fale; pare com o atalho | HUD mostra `Nada foi ouvido` (o ruído não conta como fala); as barras do HUD continuam reagindo ao ruído | ☐ | ☐ | ☐ |
| 8.10 | Sem ONNX Runtime (passo 8.3 no Windows; Linux sem `ORT_DYLIB_PATH`): repita o 3.6 | Para sozinho pelo RMS como antes, sem erro no HUD; no terminal `[pointspeak] VAD: ONNX Runtime indisponível; parada por silêncio pelo RMS` | ☐ | ☐ | ☐ |
| 8.11 | (Dev, opcional) Vocabulário real do Whisper ONNX: baixe `vocab.json` e `added_tokens.json` de `onnx-community/whisper-tiny` e de `onnx-community/whisper-large-v3-turbo` (URLs no `ONNX_CATALOG`) e rode `WHISPER_VOCAB_DIR=<pasta> cargo test -p pointspeak-core real_vocab -- --ignored --nocapture` para cada um | Passa nos dois; imprime `pt=50267` com 99 idiomas (tiny) e 100 (large-v3-turbo) | — | ☐ | — |

### 8.A DirectML no Windows (engine Whisper ONNX)

Num PC com GPU ou NPU DirectX 12 (8.2 com `NPU (DirectML)` disponível) e o app aberto com o stderr num arquivo (`Start-Process … -RedirectStandardError`, veja
**Logs no Windows** no começo). O DirectML tenta a NPU primeiro e, se ela não iniciar ou não abrir o modelo, a GPU; só depois cai para a CPU.

| # | Passo | Esperado | Win | X11 | Wayland |
| --- | --- | --- | --- | --- | --- |
| 8.12 | Hardware → `NPU (DirectML)` (ou `Automático`, que escolhe o DirectML primeiro) e abra Modelos | A lista mostra a variante ONNX: modelos com só o `.bin` aparecem como `Baixar`; tamanhos iguais aos do ggml | ☐ | — | — |
| 8.13 | Baixe o `base` e clique em `Usar este` | Progresso único até `Pronto` (soma dos 4 arquivos); pasta `models\onnx-base\` com `encoder_model_fp16.onnx`, `decoder_model_merged_fp16.onnx`, `vocab.json`, `added_tokens.json` | ☐ | — | — |
| 8.14 | Dite uma frase em PT e outra em EN | Texto colado; terminal `[pointspeak] transcrição: ONNX Runtime em DirectML · NPU` (PC com NPU) ou `… em DirectML · GPU` (sem NPU); rodapé `Pronto · base · DirectML · NPU` (ou `· GPU`); Hardware mostra `Em uso agora: DirectML · NPU` e nenhum aviso de fallback para a CPU | ☐ | — | — |
| 8.15 | Com o `large-v3-turbo` ativo **e a variante ONNX dele baixada** (Modelos com DirectML escolhido mostra `Ativo`, não `Baixar`; pasta `models\onnx-large-v3-turbo\` com os 5 arquivos), Gerenciador de Tarefas → Desempenho durante um ditado longo (~20 s) | Num PC com NPU, o gráfico da **NPU** sobe na transcrição (rodapé `· DirectML · NPU`); sem NPU, o da GPU. Se o gráfico da NPU ficar zerado, confira o rodapé e o terminal: `· CPU` = variante ONNX ausente ou fallback (8.16), `· GPU` = a NPU falhou (8.21). Anote o tempo entre soltar o atalho e colar, comparado com a CPU (8.18) | ☐ | — | — |
| 8.16 | **Fallback (variante ausente):** em `CPU`, baixe o `tiny` (`.bin`) e clique em `Usar este`; volte ao `NPU (DirectML)` sem baixar a variante ONNX do `tiny` e dite | Ditado funciona na CPU; terminal `… whisper.cpp em CPU (fallback: Modelo tiny para ONNX não baixado)`; Hardware mostra o aviso "A aceleração NPU (DirectML) não iniciou. O PointSpeak está usando a CPU. Motivo: Modelo tiny para ONNX não baixado"; rodapé `· CPU` | ☐ | — | — |
| 8.17 | **Fallback (modelo ONNX inválido):** feche o app, sobrescreva o começo do `models\onnx-base\encoder_model_fp16.onnx` mantendo o tamanho (PowerShell: `$f=[IO.File]::Open("<caminho>",'Open'); $f.Write((New-Object byte[] 64),0,64); $f.Close()`), com o `ggml-base.bin` também baixado (em `CPU`), abra e dite | Ditado funciona na CPU; com DirectML ainda indicado como disponível, Hardware mostra o detalhe do dispositivo detectado e o aviso com a causa real `DirectML não carregou o modelo ONNX: …`; rodapé mostra `· CPU`. Clique `Tentar de novo`: a lista e o estado atual do motor são recarregados e o aviso permanece enquanto o modelo continuar inválido. Depois ative outro modelo, remova o `base` (variante ONNX) e baixe de novo | ☐ | — | — |
| 8.18 | Troque o backend para `CPU` | A lista de Modelos volta à variante ggml sozinha; o próximo ditado carrega o whisper.cpp (`transcrição: whisper.cpp em CPU`), sem aviso de fallback | ☐ | — | — |
| 8.19 | Com a variante ONNX do modelo ativo ausente (8.16), baixe-a com o DirectML escolhido | Ao terminar, a bandeja passa por `Carregando modelo…` e o próximo ditado sai no DirectML (o fallback some) | ☐ | — | — |
| 8.21 | **NPU → GPU:** num PC com NPU em que a NPU não abre o modelo (ou sem NPU), dite com o `NPU (DirectML)` | Ditado funciona acelerado na GPU; terminal `[pointspeak] engine ONNX (DirectML · NPU): …` e `transcrição: ONNX Runtime em DirectML · GPU (NPU não … Usando a GPU.)`; Hardware mostra o aviso informativo "A transcrição continua acelerada pelo DirectML. NPU …. Usando a GPU." (não o aviso de CPU) e `Em uso agora: DirectML · GPU`; rodapé `· DirectML · GPU`. Sem NPU nenhuma, o motivo é `NPU não iniciou: …` | ☐ | — | — |
| 8.22 | **Verificação de sanidade:** com `NPU (DirectML)` escolhido, abra o app com o stderr num arquivo, troque de modelo (ou reabra) e use **Microfone → Testar transcrição** com uma frase em PT | Terminal `[pointspeak] verificação de sanidade (DirectML · NPU): correlação 0.99…` (≥ 0,95) e o texto certo no teste. Se a NPU calcular errado: `engine ONNX (DirectML · NPU): Diverged("resultado diverge da CPU …")`, a GPU assume (`deviceFallbackReason` `NPU reprovou na verificação: …. Usando a GPU.`) e o diagnóstico do teste mostra `DirectML · GPU`; anote a correlação da NPU. **Quando o DirectML reprovar (NPU ou GPU), copie para a issue** todas as linhas `[pointspeak] verificação de sanidade (DirectML · NPU) passo 0 … 3` e `(DirectML · GPU) passo 0 … 3`, as duas `… : correlação …`, as `engine ONNX (DirectML · …): Diverged(…)` e a `transcrição: …`. O passo 0 é a saída do encoder e os passos 1 a 3 os logits do decoder (o 1º sem KV cache); cada linha traz correlação, `tamanho <dispositivo>/<CPU>`, `NaN`, `±inf`, `constante` e min/máx/média. Leitura rápida: NaN/±inf no passo 0 = encoder estoura (fp16?); passo 0 ≈ 1,00 e passo 1 ruim = decoder; só os passos 2–3 ruins = KV cache (`use_cache_branch`); o motivo curto (`resultado diverge da CPU (logits com NaN; correlação …)`, `(encoder diverge; …)`, `(decoder diverge no passo 2; …)`) resume | ☐ | — | — |
| 8.23 | **Fallback para a engine ONNX na CPU (task #829):** com `NPU (DirectML)` escolhido e só a variante ONNX do modelo ativo baixada (sem o `.bin`), num PC em que a NPU e a GPU reprovam na verificação (8.22) ou o DirectML não inicia, use **Microfone → Testar transcrição** e dite | O modelo carrega e o texto sai certo, na CPU; terminal `[pointspeak] transcrição: ONNX Runtime em CPU (fallback: DirectML reprovou na verificação: …)`; Hardware mostra o aviso "A aceleração NPU (DirectML) não iniciou. O PointSpeak está usando a CPU. Motivo: DirectML reprovou na verificação: …"; rodapé `· CPU`. Com o `.bin` também baixado, a CPU usa o whisper.cpp (`whisper.cpp em CPU (fallback: …)`) | ☐ | — | — |
| 8.24 | **NPU Intel pelo OpenVINO (2026-10-19):** num Core Ultra em que o cartão `NPU (DirectML)` mostra `Sem NPU`, escolha `OpenVINO (Intel)` com o `large-v3-turbo` (variante ONNX com os 5 arquivos, incluindo `decoder_with_past_model_fp16.onnx`), abra o app com o stderr num arquivo e espere a carga | O cartão mostra `NPU detectada · Intel`; na 1ª carga com o modelo, Hardware mostra "Preparando o OpenVINO…" e a carga demora (≈17 min no large-v3-turbo; aparece `models\onnx-large-v3-turbo\openvino-cache\*.blob`); terminal `verificação de sanidade (OpenVINO · NPU): correlação 0.99…` e `transcrição: ONNX Runtime em OpenVINO · NPU`; rodapé `· OpenVINO · NPU` e `Em uso agora: OpenVINO · NPU`. Reabra o app: a carga leva segundos. Dite ~15 s: o gráfico da NPU sobe e o texto sai certo; anote o tempo | ☐ | — | — |
| 8.25 | **DirectML com KV cache (2026-10-19):** com `NPU (DirectML)` escolhido num PC em que o DirectML roda na GPU, abra o app com o stderr num arquivo | `verificação de sanidade (DirectML · GPU) passo 2 (decoder, com KV cache): correlação 1.00…` (sem NaN) e `transcrição: ONNX Runtime em DirectML · GPU`, sem o aviso de CPU. Um modelo ONNX baixado antes desta versão aparece como `Baixar` em **Modelos**: o download só traz o `decoder_with_past_model_fp16.onnx` (os outros arquivos são reaproveitados) | ☐ | — | — |
| 8.26 | **Verificação uma vez só:** com `OpenVINO (Intel)` ou `NPU (DirectML)`, abra o app duas vezes com o stderr num arquivo | 1ª vez: as linhas `verificação de sanidade (…) passo 0 … 3` e o arquivo `%APPDATA%\com.pointspeak.app\verified-devices.json` criado; 2ª vez: `verificação de sanidade (…): aprovada antes; só aquecimento` e a carga bem mais rápida | ☐ | — | — |
| 8.27 | **Liberar a memória:** Hardware → `Liberar a memória do modelo` = `Depois de 5 minutos sem uso`; espere 5–6 min sem ditar olhando o Gerenciador de Tarefas | A memória do PointSpeak cai para algumas centenas de MB; rodapé `Pronto · <modelo> · memória liberada (carrega ao ditar)`. Dite uma frase de ~5 s: o texto sai certo (a carga acontece enquanto você fala; anote o tempo até colar) | ☐ | — | — |
| 8.28 | **Pouca memória:** feche o app, defina `POINTSPEAK_FREE_MEMORY_MB=4000` e abra o PointSpeak com o `large-v3-turbo` no OpenVINO | O modelo não carrega (a memória do processo fica perto de 150 MB, sem pico) e o rodapé mostra `Ditado indisponível: Pouca memória livre para carregar o modelo: precisa de cerca de 7,8 GB e há 3,9 GB livres. …`. Sem a variável, carrega normal | ☐ | — | — |
| 8.29 | **Encoder pré-compilado (OpenVINO):** com `OpenVINO (Intel)` e o `large-v3-turbo`, abra o app pela 1ª vez depois de instalar (ou apague `openvino-cache`) e espere a carga; depois feche e abra de novo | 1ª vez: `encoder pré-compilado (OpenVINO · NPU) gravado em …\openvino-cache\encoder_npu.onnx` no log (pode levar ~15 min) e nenhum `*.blob` na pasta `openvino-cache`. 2ª vez: carga em poucos segundos e memória do PointSpeak perto de 3 GB, sem pico acima de ~3,5 GB | ☐ | — | — |
| 8.20 | (Dev, opcional) Linux, engine ONNX na CPU EP: `./scripts/fetch-onnxruntime.sh linux`, baixe a pasta `onnx-tiny` (URLs no `ONNX_CATALOG`) e rode `ORT_DYLIB_PATH=$PWD/target/onnxruntime/linux-x64/libonnxruntime.so cargo run -p pointspeak --release --example transcribe -- --backend cpu <pasta onnx-tiny> jfk.wav en` | Mesmo texto do whisper.cpp (`… ggml-tiny.bin jfk.wav en`); `--backend directml` no Linux sai com `DirectML is not supported in this build of ONNX Runtime` | — | ☐ | — |

## 9. Riscos aceitos (revisão de segurança, task #807)

Comportamentos conhecidos que a 1.0.0 entrega assim de propósito. Confira cada um e marque se aceita; os
detalhes estão em `docs/tecnologia.md` (**Segurança e privacidade**).

| # | Risco | O que conferir | Aceito |
| --- | --- | --- | --- |
| 9.1 | **Sem assinatura de código:** o .exe, o .deb e o AppImage não são assinados; o SmartScreen avisa (1.1) e a integridade depende do canal por onde o arquivo chega | Compare o SHA-256 do arquivo recebido com o de `docs/tecnologia.md` | ☐ |
| 9.2 | **Clipboard compartilhado por ~400 ms (Windows) / ~50 ms (Linux):** durante a colagem outro app que leia o clipboard vê o texto ditado; gerenciadores de clipboard que ignoram a dica de histórico podem guardá-lo (4.6) | Se usa um gerenciador de clipboard, veja se a frase ditada fica guardada nele | ☐ |
| 9.3 | **`ORT_DYLIB_PATH`:** quem controla as variáveis de ambiente da sua conta pode fazer o app carregar outra biblioteca ONNX Runtime (a variável existe para desenvolvimento; caminho relativo é ignorado) | Nada a testar; risco pressupõe acesso à sua conta | ☐ |
| 9.4 | **Instalação por usuário (Windows):** a pasta `%LOCALAPPDATA%\PointSpeak` é gravável pelo próprio usuário, como em todo app instalado por usuário; as DLLs do ONNX Runtime são carregadas dela | Confira em 8.1 que as DLLs estão nessa pasta | ☐ |
| 9.5 | **CPU com AVX2 obrigatória:** em CPU sem AVX2/FMA o app fecha ao transcrever (o .exe e os pacotes Linux são compilados para AVX2) | Se tiver um PC antigo, anote o comportamento | ☐ |
| 9.6 | **Wayland:** o HUD fica fixo no centro-inferior (5.3), o atalho global depende do compositor (6.6) e a colagem depende de `wtype`/`ydotool` (4.5) | Itens 4.5, 5.3 e 6.6 | ☐ |
| 9.7 | **Pasta de logs vazia:** os logs `[pointspeak] …` saem só no terminal (stderr); `Sobre → Abrir pasta de logs` abre a pasta, mas o app ainda não grava arquivo de log | Abra o app por um terminal (Windows: **Logs no Windows**, no começo) quando precisar do motivo de um erro | ☐ |
| 9.8 | **Callback de áudio aloca memória** por bloco (conversão de formato): não bloqueia a captura, mas não é livre de alocação | Nada a testar; só desempenho em máquina muito carregada | ☐ |
