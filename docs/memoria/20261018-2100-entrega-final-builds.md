# Builds finais (instaladores) num worktree novo

tipo: armadilha
origem: task #808 · claude · 2026-10-18

- Worktree novo não tem `target/`: `./scripts/build-windows.sh` e depois `./scripts/build-linux.sh` do zero levaram 4 min 07 s + 4 min 40 s (8 núcleos; caches `~/.cache/cargo-xwin` e `~/.cache/tauri` já existiam). Rode os dois em sequência, em background, e escreva a doc enquanto isso.
- Espaço: `target/x86_64-pc-windows-msvc/` 1,3 GB + `target/release/` 2,2 GB; os checks somam ~7,6 GB de `target/debug`. Com ~26 GB livres não foi preciso limpar nada.
- Versão anda em três lugares: `Cargo.toml` (workspace), `src-tauri/tauri.conf.json` e `package.json` (`npm version x.y.z --no-git-tag-version` atualiza o lock); depois `cargo update -w --offline` atualiza o `Cargo.lock`. O mock de `getVersion` em `src/test/tauri-mock.ts` e o teste da seção Sobre usam a string da versão.
- O `pointspeak.exe` de release é GUI (`windows_subsystem = "windows"`): o stderr não aparece no PowerShell. Para ver os `[pointspeak] …`: `Start-Process <exe> -RedirectStandardError <arquivo>`. O app não grava arquivo de log (a pasta de logs fica vazia).
