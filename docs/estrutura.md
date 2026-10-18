# Estrutura: Pointspeak

Onde fica cada coisa (atualize quando criar ou mover pastas).

- `.gitignore`: o que o git ignora (segredos, `node_modules/`, `dist/`, `target/`, `.runner/`)
- `AGENTS.md`: instruções para agentes de IA e pessoas
- `CLAUDE.md`: aponta para o `AGENTS.md`
- `README.md`: apresentação do projeto
- `docs/`: produto, tecnologia e esta estrutura
  - `docs/design/`: visual do app, mantido pela persona `designer`
    - `DESIGN.md`: tokens, medidas do HUD, animações, seções das Configurações, componentes, menu da bandeja
    - `README.md`: como abrir as telas, mapa estado/seção → arquivo, guia React/Tailwind
    - `telas/hud.html`, `telas/settings.html`: protótipos estáticos (abrem no navegador, sem build)
  - `docs/memoria/`: aprendizados entre tasks
- `scripts/build-windows.sh`: gera o instalador Windows (.exe NSIS) por compilação cruzada
- `scripts/setup-linux.sh`: instala as libs de sistema do Linux (apt, idempotente)
- `scripts/setup-windows-cross.sh`: prepara a compilação cruzada para Windows (nsis, lld/llvm, target msvc, cargo-xwin)
- `package.json`, `package-lock.json`: frontend e scripts `lint`, `test`, `build`
- `hud.html`, `settings.html`: páginas de entrada das duas janelas do Tauri
- `vite.config.ts`: build (multi-página) e config do Vitest
- `tsconfig.json`, `eslint.config.js`: TypeScript strict e ESLint
- `src/`: frontend React
  - `src/hud/`: janela do HUD (`HudOverlay.tsx`, placeholder que segue o evento `hud://state-changed`)
  - `src/settings/`: janela de configurações (`Settings.tsx`, placeholder)
  - `src/test/`: setup do Vitest e mock de `@tauri-apps/api`
  - `src/styles.css`: entrada do Tailwind
- `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml`: workspace Cargo (`src-tauri` e `crates/pointspeak-core`)
- `crates/pointspeak-core/`: núcleo em Rust puro, testável sem sistema
  - `src/hud.rs`: máquina de estados do HUD (`Idle`, `Recording`, `Transcribing`, `Success`, `Error`)
- `src-tauri/`: app Tauri v2 (crate `pointspeak`)
  - `tauri.conf.json`: identificador, janelas `settings` e `hud`, bundle (nsis, deb, appimage; WebView2 `embedBootstrapper` no Windows)
  - `capabilities/`: permissões das janelas
  - `icons/`: ícones gerados por `npx tauri icon src-tauri/app-icon.png -o src-tauri/icons`
  - `src/lib.rs`: montagem do app (plugins, estado, janelas); `src/tray.rs`: bandeja; `src/main.rs`: ponto de entrada
