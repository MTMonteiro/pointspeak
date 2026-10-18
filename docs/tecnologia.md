# Tecnologia: Pointspeak

**Tipo de software:** desktop (Windows e Linux), utilitário de bandeja.

## Stack
- **Linguagens:** Rust (edition 2021) e TypeScript (strict).
- **App desktop:** Tauri v2 (`src-tauri/`), com `tauri-plugin-global-shortcut` e ícone na bandeja (feature `tray-icon`).
- **Frontend:** React 18 + Vite + Tailwind CSS v4 (plugin `@tailwindcss/vite`). Uma página por janela: `hud.html` e `settings.html`.
- **Núcleo:** `crates/pointspeak-core`, Rust puro sem dependências de sistema (hoje: máquina de estados do HUD).
- **Planejado para as próximas fases** (ainda não adicionado): `cpal` + `rubato` (áudio), `whisper-rs` (whisper.cpp), `ort` (ONNX Runtime, NPU/DirectML), `arboard` + `enigo` (+ `wtype`/`ydotool` no Wayland), `x11rb` / crate `windows`, `reqwest` (rustls) + `sha2` (download de modelos).
- **Testes:** `cargo test` + `clippy` + `rustfmt`; Vitest + Testing Library (jsdom), com mock de `@tauri-apps/api` em `src/test/tauri-mock.ts`.
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
- **Limitações:** só o esqueleto (Tauri + bandeja + atalho global) é validado. whisper.cpp (`whisper-rs`, C++/cmake) e ONNX Runtime (`ort`) ainda não entram; o build cruzado deles será validado nas fases de transcrição e de aceleração, que precisam manter este script funcionando. O .exe só é testado de verdade num Windows (manual, pelo dono).

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
