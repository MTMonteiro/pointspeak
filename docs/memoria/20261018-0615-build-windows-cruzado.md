# Build cruzado do .exe Windows neste runner

tipo: armadilha
origem: task #754 · claude · 2026-10-18

- `./scripts/setup-windows-cross.sh` já rodado neste runner (nsis, lld, llvm, clang, target msvc, cargo-xwin 0.23.1).
- `file` no `PointSpeak_*_x64-setup.exe` mostra **PE32 / Intel 80386 / Nullsoft**: o stub do NSIS é sempre 32 bits. O app de 64 bits (PE32+) é o `target/x86_64-pc-windows-msvc/release/pointspeak.exe`.
- Avisos que não são erro: `LNK4099` (pdb do libcmt), `Cross-platform compilation is experimental`, `skipping signing`, makensis `warning 5202`.
- O `embedBootstrapper` e o `nsis_tauri_utils.dll` são baixados da rede pelo tauri-cli durante o bundle: o build Windows precisa de internet (os checks não).
- Espaço: ~0,8 GB em `target/x86_64-pc-windows-msvc`, ~0,45 GB em `target/release` (deps de host) e ~1,1 GB em `~/.cache/cargo-xwin`. Build do zero ~4 min; incremental, segundos.
