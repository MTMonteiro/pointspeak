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
- **Infra (Windows):** compilação cruzada com `cargo-xwin` + target `x86_64-pc-windows-msvc` + NSIS + lld/llvm, gerando o instalador `.exe`. Script e processo: ainda não definidos (fases seguintes).
- **Toolchain:** `rust-toolchain.toml` usa o canal `stable` e garante os componentes `rustfmt` e `clippy`.

## Setup (Linux, Debian/Ubuntu)
1. Rust via rustup e Node.js 22 com npm.
2. `./scripts/setup-linux.sh`: instala webkit2gtk-4.1, gtk3, appindicator, librsvg, alsa, xdo, ssl, x11/xcb, pkg-config, build-essential, cmake, clang/libclang. Idempotente: sem pacote faltando, não chama o `apt-get`.
3. `npm ci`.

## Como rodar
- App em desenvolvimento: `npm run tauri dev` (Vite na porta 1420).
- Binário Linux: `npm run build && cargo build -p pointspeak`.
- Pacotes (deb/AppImage): `npm run tauri build`.

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
