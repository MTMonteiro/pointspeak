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
- Transcrever um WAV sem abrir o app: `cargo run -p pointspeak --release --example transcribe -- <modelo.bin> <audio.wav> [pt|en|auto]`. Modelos e WAVs de teste ficam fora do repositório (ou em `target/`); nenhum teste dos checks usa modelo.
- Colar um texto no app em foco pelo injector (precisa de tela; espera 3 s): `cargo run -p pointspeak --example inject -- "texto"`. Nenhum teste dos checks toca o clipboard real ou simula teclas.
- Instalador Windows (compilação cruzada, neste Linux): `./scripts/setup-windows-cross.sh` (uma vez; nsis, lld, llvm, clang, ninja, target `x86_64-pc-windows-msvc`, cargo-xwin) e `./scripts/build-windows.sh`. Sai em `target/x86_64-pc-windows-msvc/release/bundle/nsis/PointSpeak_<versão>_x64-setup.exe`. Sem assinatura (SmartScreen avisa); WebView2 via bootstrapper embutido. Detalhes e limitações em `docs/tecnologia.md`.
- Validação manual (tela, microfone, atalho, clipboard) no Windows e no Linux X11/Wayland: `docs/checklist-manual.md`.
- Código Windows-only fica atrás de `cfg(windows)` e não pode quebrar os checks Linux. Fase que adicionar dependência nativa (whisper-rs, ort) roda o `build-windows.sh` antes de entregar.
- Nenhum teste pode depender de tela, microfone ou rede; lógica pura vai em `crates/pointspeak-core`.
- Design e UI: o visual segue `docs/design/` (DESIGN.md e telas); a persona `designer` mantém essa pasta.
- Mudou o plano, a stack, os comandos ou a organização das pastas: atualize `docs/` e este arquivo no mesmo PR.
- Nunca versione segredos (`.env`, chaves, tokens); use variáveis de ambiente.
<!-- fabrica:fim -->
