# Tecnologia: Pointspeak

**Tipo de software:** desktop (Windows e Linux), utilitário de bandeja.

## Stack
- **Linguagens:** Rust (edition 2021) e TypeScript (strict).
- **App desktop:** Tauri v2 (`src-tauri/`), com `tauri-plugin-global-shortcut` e ícone na bandeja (feature `tray-icon`).
- **Frontend:** React 18 + Vite + Tailwind CSS v4 (plugin `@tailwindcss/vite`). Uma página por janela: `hud.html` e `settings.html`.
- **Núcleo:** `crates/pointspeak-core`, Rust puro sem dependências de sistema (`serde`, `thiserror`, `rubato` e `sha2`): máquina de estados do HUD, contrato dos eventos, configuração, lógica do atalho (PTT/Toggle), pipeline de áudio e Model Manager (catálogo e download verificado).
- **Configuração:** `AppConfig` em `app_config_dir()/config.json` (JSON camelCase; `~/.config/com.pointspeak.app/` no Linux, `%APPDATA%\com.pointspeak.app\` no Windows). Escrita atômica (arquivo `.tmp` + rename). Arquivo ausente ou corrompido carrega os padrões (com log no stderr); campo inválido volta ao padrão e os demais são mantidos. Comandos Tauri: `get_config` e `set_config(config)` (valida, grava e devolve o erro em PT-BR).
- **Eventos backend → HUD:** `hud://state-changed` com `{ state: "idle|recording|transcribing|success|error", message?: string }` (`message` só no erro, texto curto do `docs/design/DESIGN.md` §3) e `hud://audio-level` com `{ level: 0..1 }`. Constantes e payloads em `pointspeak-core/src/events.rs`.
- **Áudio (em memória, nunca vai para o disco):** `cpal` 0.18 no `src-tauri/src/audio.rs` (ALSA no Linux via `libasound2-dev`, WASAPI no Windows) e `rubato` 5 no núcleo (`pointspeak-core/src/audio.rs`).
  - Captura: `AudioRecorder::from_config(&AppConfig).start(device, on_level, on_auto_stop)` → `RecordingHandle::stop() -> Vec<f32>` (16 kHz, mono). Usa o formato padrão do dispositivo (f32/i16/u16). O callback do cpal só converte para f32 e faz `try_send` num canal limitado; o processamento roda numa thread própria.
  - Pipeline (`CapturePipeline`): downmix para mono (média) → resample FFT síncrono (`rubato::Fft`, blocos de 1024 quadros, atraso do filtro descartado e resto processado no fim; saída = `ceil(n × 16000 / taxa)`) → RMS a cada 30 ms → `level` 0..1 em escala dB (-60 dB → 0, -10 dB → 1) → `SilenceDetector` → buffer limitado a `maxRecordingSecs`.
  - Parada por silêncio (`autoStopOnSilence`): limiar fixo de -40 dBFS de RMS por janela; só arma depois de 90 ms contínuos acima do limiar (houve fala) e dispara após `silenceTimeoutMs` seguidos abaixo dele. Sem relógio real: a duração vem do número de amostras. O Silero VAD substitui isso na fase 6.
  - Ao atingir `maxRecordingSecs` o buffer para de crescer e o pipeline sinaliza parada; microfone desconectado também chama `on_auto_stop` (e `RecordingHandle::device_lost()` fica `true`).
  - Erros (`AudioError`) viram `ErrorKind::MicrophoneUnavailable` no HUD e texto PT-BR na interface. Comando Tauri `list_microphones` → `[{ name, isDefault }]` (o `name` é o valor de `AppConfig.microphone`).
- **Model Manager (download de modelos):** catálogo, estados e download verificado em `pointspeak-core/src/models.rs` (`sha2`); transporte e comandos em `src-tauri/src/models.rs` (`reqwest` 0.12 blocking com `rustls-tls`/ring, sem OpenSSL nem aws-lc; o Tauri traz o reqwest 0.13 por conta própria).
  - Catálogo: `ggml-tiny.bin`, `ggml-base.bin`, `ggml-small.bin`, `ggml-large-v3-turbo.bin` de `https://huggingface.co/ggerganov/whisper.cpp/resolve/main/<arquivo>`, com tamanho em bytes e SHA-256 tirados do `oid sha256` do LFS (`https://huggingface.co/api/models/ggerganov/whisper.cpp/tree/main`). Textos e tamanhos exibíveis do `DESIGN.md` §4.2; `base` é o recomendado.
  - Pasta: `app_data_dir()/models` (`~/.local/share/com.pointspeak.app/models` no Linux, `%APPDATA%\com.pointspeak.app\models` no Windows). O scan (na inicialização e a cada `list_models`) só confere o tamanho do arquivo; o hash é conferido no download.
  - Download (`download_verified`): consome um `Read`, grava em `<arquivo>.part`, calcula o SHA-256 enquanto grava, avisa o progresso a cada 1 % ou 250 ms, checa a flag de cancelamento a cada bloco de 64 KiB e só renomeia para o nome final com tamanho e hash certos. Em erro ou cancelamento o `.part` é apagado. Mensagens: `Download falhou: conexão perdida` (leitura falhou ou o stream acabou antes), `Arquivo corrompido (SHA-256 não confere)` (hash ou tamanho a mais), `Não foi possível gravar o modelo: …` (disco). Uma thread por download; timeout de 30 s para conectar e para cada leitura.
  - Estados por modelo (`ModelState`, JSON `{ "state": … }`): `notDownloaded`, `downloading` (`done`, `total`), `verifying`, `ready`, `active` (o `AppConfig.model`, se o arquivo existe), `failed` (`reason`).
  - Comandos Tauri: `list_models` → `{ models: [{ id, fileName, sizeBytes, sha256, displaySize, description, recommended, state, done?, total?, reason? }], dir, usedBytes }`; `download_model({ id })` (volta na hora; um por vez por modelo); `cancel_download({ id })`; `remove_model({ id })` (recusa o ativo com "Troque o modelo ativo antes de remover" e o que está baixando); `set_active_model({ id })` (precisa estar `ready`; grava `AppConfig.model` e devolve a config). `open_models_dir` fica para a fase das telas.
  - Evento `models://progress` com `{ id, state, done, total, reason? }` a cada mudança (constante e payload em `pointspeak-core/src/events.rs`).
- **Planejado para as próximas fases** (ainda não adicionado): `whisper-rs` (whisper.cpp), `ort` (ONNX Runtime, NPU/DirectML), `arboard` + `enigo` (+ `wtype`/`ydotool` no Wayland), `x11rb` / crate `windows`.
- **Testes:** `cargo test` + `clippy` + `rustfmt` (`tempfile` como dev-dependency do núcleo e do `src-tauri` para testar a persistência e o download); Vitest + Testing Library (jsdom), com mock de `@tauri-apps/api` em `src/test/tauri-mock.ts`.
- **Lint:** ESLint flat config (`eslint.config.js`) com typescript-eslint e react-hooks, sem warnings.
- **Infra (Windows):** compilação cruzada com `cargo-xwin` + target `x86_64-pc-windows-msvc` + NSIS + lld/llvm, gerando o instalador `.exe` (veja **Instalador Windows** abaixo).
- **Toolchain:** `rust-toolchain.toml` usa o canal `stable` e garante os componentes `rustfmt` e `clippy`.

## Setup (Linux, Debian/Ubuntu)
1. Rust via rustup e Node.js 22 com npm.
2. `./scripts/setup-linux.sh`: instala webkit2gtk-4.1, gtk3, appindicator, librsvg, alsa, xdo, ssl, x11/xcb, pkg-config, build-essential, cmake, clang/libclang. Idempotente: sem pacote faltando, não chama o `apt-get`.
3. `npm ci`.

## Como rodar
- App em desenvolvimento: `npm run tauri dev` (Vite na porta 1420).
- Binário Linux: `npm run build && cargo build -p pointspeak`.
- Pacotes (deb/AppImage): `npm run tauri build`.

## Instalador Windows (.exe NSIS, compilação cruzada)
O instalador Windows sai deste Linux pelo caminho documentado do Tauri v2 (cargo-xwin + `x86_64-pc-windows-msvc` + NSIS). Não entra nos checks (pesado demais); rode nas etapas que mexem em dependência nativa e no empacotamento.

1. Pré-requisitos (uma vez por máquina, depois do `setup-linux.sh`): `./scripts/setup-windows-cross.sh`. Idempotente: instala `nsis lld llvm clang` pelo apt, `rustup target add x86_64-pc-windows-msvc` e `cargo install --locked cargo-xwin`.
2. Gerar: `./scripts/build-windows.sh`. Roda `npm ci` se faltar `node_modules`, depois `npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis`, e imprime o caminho do instalador.
3. Saída: `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe` (target dir do workspace, na raiz). Confira com `file`: o instalador sai como `PE32 executable (GUI) Intel 80386 ... Nullsoft Installer self-extracting archive` (o stub do NSIS é sempre 32 bits, normal) e o app dentro dele, `target/x86_64-pc-windows-msvc/release/pointspeak.exe`, como `PE32+ executable (GUI) x86-64, for MS Windows`. O esqueleto gera um instalador de ~2,9 MiB.

- **WebView2:** `bundle.windows.webviewInstallMode = embedBootstrapper` no `tauri.conf.json`: o bootstrapper vai dentro do instalador, que instala o WebView2 se faltar. O bootstrapper ainda baixa o runtime da Microsoft; num Windows 10/11 atualizado o WebView2 já vem instalado e nada é baixado.
- **Avisos esperados no build:** `Cross-platform compilation is experimental`, `skipping signing the installer`, `lld-link: Cannot use debug info for 'libcmt.lib(...)' [LNK4099]` (o CRT da Microsoft não traz os .pdb) e `warning 5202: -OUTPUTCHARSET` do makensis. Nenhum impede o instalador.
- **Sem assinatura de código:** o Windows SmartScreen mostra "O Windows protegeu o computador" ao abrir o .exe. É esperado; clique em "Mais informações" → "Executar assim mesmo".
- **Cache do xwin:** o cargo-xwin baixa o CRT do MSVC e o Windows SDK para `~/.cache/cargo-xwin` (fora do repositório; não versione). O primeiro build baixa isso; os seguintes reaproveitam.
- **Espaço em disco:** medido no esqueleto (task #754): `target/x86_64-pc-windows-msvc/` ~0,8 GB, `target/release/` ~0,45 GB (build scripts e proc-macros do host, compilados em release) e `~/.cache/cargo-xwin` ~1,1 GB (só uma vez por máquina). Total ~2,4 GB, fora o `target/debug` dos checks (~4 GB). Primeiro build: ~4 min (inclui baixar o CRT/SDK). Se faltar espaço, apague só `target/x86_64-pc-windows-msvc/debug` (se existir) ou o `target/x86_64-pc-windows-msvc/` inteiro, nunca o cache do xwin à toa.
- **Limitações:** validado o esqueleto (Tauri + bandeja + atalho global) e, desde a task #763, o `cpal` (WASAPI); desde a task #764, o `reqwest` com `rustls`/`ring` (o instalador passou a ~3,5 MiB). whisper.cpp (`whisper-rs`, C++/cmake) e ONNX Runtime (`ort`) ainda não entram; o build cruzado deles será validado nas fases de transcrição e de aceleração, que precisam manter este script funcionando. O .exe só é testado de verdade num Windows (manual, pelo dono).

## Como rodar os testes
Na ordem (o `dist/` do `npm run build` é exigido pelo `generate_context!` do Tauri ao compilar o Rust):

```sh
npm ci
npm run lint
npm test                 # no runner: RUNNER_TEST_MAX_WORKERS=2 npm test
npm run build            # tsc --noEmit && vite build
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- Testes afetados enquanto itera: `npx vitest run --changed origin/main`; só o núcleo: `cargo test -p pointspeak-core`.
- O `vite.config.ts` lê `RUNNER_TEST_MAX_WORKERS` (número ou percentual; padrão `50%`).
- Nenhum teste usa tela, microfone ou rede.
