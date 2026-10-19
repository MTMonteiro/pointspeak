# AGENTS.md: Pointspeak

Instruções para agentes de IA (Claude Code, Codex, OpenCode) e pessoas que trabalham neste repositório.

<!-- fabrica:inicio -->
## Fábrica de software
Projeto da ideia **Pointspeak**, criado ou importado pela Fábrica de software do runner. O plano completo vive no runner;
o que um agente ou uma pessoa precisa para trabalhar aqui fica neste repositório:
- `docs/produto.md`: o que é o produto, para quem e o escopo.
- `docs/tecnologia.md`: stack, motivos e como rodar os testes.
- `docs/estrutura.md`: onde fica cada coisa.

### Como trabalhar
- Leia os três arquivos de `docs/` antes de mudar o código.
- Setup do Linux (uma vez por máquina, Debian/Ubuntu): `./scripts/setup-linux.sh` (idempotente; usa `sudo apt-get`).
- Comandos de verificação (precisam passar **nesta ordem**, o `npm run build` gera o `dist/` que o `generate_context!` do Tauri exige):
  1. `npm ci`
  2. `npm run lint`
  3. `npm test` (no runner: `RUNNER_TEST_MAX_WORKERS=2 npm test`)
  4. `npm run build`
  5. `cargo fmt --all -- --check`
  6. `cargo clippy --workspace --all-targets -- -D warnings`
  7. `cargo test --workspace`
- Binário Linux: `cargo build -p pointspeak` (gera `target/debug/pointspeak`). O build Windows (.exe NSIS) não entra nos checks.
- Transcrever um WAV sem abrir o app: `cargo run -p pointspeak --release --example transcribe -- [--backend <cpu|directml|openvino|cuda>] [--repeat N] <modelo.bin | pasta onnx-<id>> <audio.wav> [pt|en|auto]` (`--repeat` mede as rodadas com a engine já carregada) (pasta = engine ONNX, com o `ORT_DYLIB_PATH`). Modelos e WAVs de teste ficam fora do repositório (ou em `target/`); nenhum teste dos checks usa modelo.
- Colar um texto no app em foco pelo injector (precisa de tela; espera 3 s): `cargo run -p pointspeak --example inject -- "texto"`. Nenhum teste dos checks toca o clipboard real ou simula teclas.
- Instalador Windows (compilação cruzada, neste Linux): `./scripts/setup-windows-cross.sh` (uma vez; nsis, lld, llvm, clang, ninja, target `x86_64-pc-windows-msvc`, cargo-xwin) e `./scripts/build-windows.sh`. Sai em `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe`. Sem assinatura (SmartScreen avisa); WebView2 via bootstrapper embutido. Detalhes e limitações em `docs/tecnologia.md`.
- Pacotes Linux (.deb e AppImage, com o `libonnxruntime.so` CPU e o Silero VAD): `./scripts/build-linux.sh` (chama o fetch, usa o overlay `src-tauri/tauri.onnxruntime-linux.conf.json` e confere o conteúdo; `POINTSPEAK_SKIP_APPIMAGE=1` gera só o .deb). Sai em `target/release/bundle/deb/PointSpeak_<versão>_amd64.deb` e `target/release/bundle/appimage/PointSpeak_<versão>_amd64.AppImage`. Não entra nos checks.
- ONNX Runtime (carregado em tempo de execução, nunca linkado): `./scripts/fetch-onnxruntime.sh [windows|linux|all]` baixa os pacotes oficiais (SHA-256 fixado) para `target/onnxruntime/`. No Linux, rode o app com `ORT_DYLIB_PATH=$PWD/target/onnxruntime/linux-x64/libonnxruntime.so`. Nenhum teste dos checks depende dele; o `build-windows.sh` já chama o fetch.
- Silero VAD (parada por silêncio): modelo versionado em `src-tauri/resources/silero_vad.onnx` (SHA-256 conferido por teste no núcleo; trocar o arquivo exige atualizar `SILERO_MODEL_SHA256` em `crates/pointspeak-core/src/vad.rs`). Sem ONNX Runtime a parada usa o RMS. Teste real opcional: `ORT_DYLIB_PATH=… [VAD_TEST_WAV=fala.wav] cargo test -p pointspeak --lib vad_real -- --ignored --nocapture`.
- Whisper ONNX (núcleo puro, sem `ort`): log-mel, vocabulário/tokens e decodificação gulosa em `crates/pointspeak-core/src/whisper_onnx/`; catálogo ONNX (`ONNX_CATALOG`, variante `ModelVariant::Onnx`, pasta `onnx-<id>/`) e download de vários arquivos em `models.rs`. Trocar um arquivo do catálogo exige tamanho e SHA-256 novos com a fonte em `docs/tecnologia.md`. Teste real opcional do vocabulário: `WHISPER_VOCAB_DIR=<pasta com vocab.json e added_tokens.json> cargo test -p pointspeak-core real_vocab -- --ignored`.
- Engine Whisper ONNX (DirectML/OpenVINO/CUDA) com fallback para a CPU (whisper.cpp se o ggml está no disco; senão a própria engine ONNX na CPU EP, com o motivo do backend acelerado): escolha e motivos puros em `crates/pointspeak-core/src/engine.rs` (`engine_kind`, `load_with_fallback`, `FallbackReason` → `fallbackReason` do `engine://status`; no DirectML e no OpenVINO a ordem NPU → GPU de `DmlDevice::ORDER`, com `device`/`deviceFallbackReason` no status; no OpenVINO só o encoder vai para a NPU e os decoders para a GPU do DirectML ou a CPU); sessões e KV cache (1º passo no `decoder_model_merged`, os seguintes no `decoder_with_past_model`: o ramo com cache do merged dá NaN no DirectML) em `src-tauri/src/onnx_whisper.rs`. O Model Manager mostra a variante (ggml/ONNX) da engine do backend efetivo. Teste real opcional: `ORT_DYLIB_PATH=… POINTSPEAK_TEST_ONNX_DIR=<pasta onnx-tiny> POINTSPEAK_TEST_WAV=<wav 16 kHz mono> cargo test -p pointspeak --lib onnx_whisper_real -- --ignored --nocapture`.
- Memória do motor e atalho de toque duplo (2026-10-19): verificação de sanidade gravada em `verified-devices.json` (depois só o aquecimento), `unloadIdleMins` + `release_if_idle`/`preload` em `src-tauri/src/transcriber.rs` (estado `released` do motor) e `Alt+Alt`/`Ctrl+Ctrl`/`Shift+Shift` pelo hook do `src-tauri/src/double_tap.rs` (só Windows; regra pura no `hotkey::DoubleTapDetector`). Detalhes em `docs/tecnologia.md`.
- Comando Tauri novo: entra no `generate_handler!` do `src-tauri/src/lib.rs`, na lista `COMMANDS` do `src-tauri/build.rs` e como `allow-<comando>` em `src-tauri/capabilities/settings.json` (o HUD não recebe comandos; `crates/pointspeak-core/tests/capabilities.rs` confere). Garantias e riscos aceitos de segurança/privacidade: `docs/tecnologia.md` (**Segurança e privacidade**).
- Validação manual (tela, microfone, atalho, clipboard) no Windows e no Linux X11/Wayland: `docs/checklist-manual.md` (roteiro de aceite 1.0.0; item novo de comportamento entra lá).
- Versão: hoje **1.0.0**, em três lugares que andam juntos: `[workspace.package].version` do `Cargo.toml`, `version` do `src-tauri/tauri.conf.json` (nome dos instaladores e tela Sobre) e `package.json` (`npm version <x.y.z> --no-git-tag-version` atualiza o lock). Ao gerar instaladores de uma versão nova, registre nome, tamanho e SHA-256 em `docs/tecnologia.md`.
- `README.md` é o guia do usuário final (instalar, primeiro uso, problemas comuns); detalhes técnicos vão em `docs/`.
- Código Windows-only fica atrás de `cfg(windows)` e não pode quebrar os checks Linux. Fase que adicionar dependência nativa (whisper-rs, ort) roda o `build-windows.sh` antes de entregar.
- Nenhum teste pode depender de tela, microfone ou rede; lógica pura vai em `crates/pointspeak-core`.
- Testes integrados do núcleo ficam em `crates/pointspeak-core/tests/`; contratos Rust↔TypeScript versionados ficam em `src/test/fixtures/rust-contracts.json` e são conferidos por `cargo test` e Vitest. Para regenerar após revisar uma mudança intencional: `cargo test -p pointspeak-core --test contract_fixtures write_rust_contract_fixtures -- --ignored`.
- Design e UI: o visual segue `docs/design/` (DESIGN.md e telas); a persona `designer` mantém essa pasta.
- A janela de Configurações fica em `src/settings/`: `Settings.tsx`, `Settings.css`, `useConfig.ts` e os controles base em `components/`.
- A seção Hardware exibe o detalhe detectado nos cartões e usa `fallbackReason` de `engine://status` no aviso de fallback; `Tentar de novo` atualiza a lista de hardware e o estado atual do motor. Se o modelo ativo não tem o formato do modo escolhido (ggml na CPU, ONNX no DirectML/OpenVINO), ela mostra o aviso com `Baixar agora (<tamanho>)` (chama o `download_model`) e o progresso, no lugar do aviso genérico de CPU.
- Mudou o plano, a stack, os comandos ou a organização das pastas: atualize `docs/` e este arquivo no mesmo PR.
- Nunca versione segredos (`.env`, chaves, tokens); use variáveis de ambiente.
<!-- fabrica:fim -->
