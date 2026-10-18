# Checks do Tauri neste runner

tipo: armadilha
origem: task #753 · claude · 2026-10-18

- O rustup do runner veio sem `rustfmt` e `clippy`; o `rust-toolchain.toml` da raiz pede os dois e o rustup baixa sozinho no primeiro `cargo`.
- `cargo clippy`/`cargo test` do `src-tauri` falham sem `dist/`: o `generate_context!` lê o `frontendDist`. Rode `npm run build` antes do cargo.
- `npx tauri icon` também gera `icons/android` e `icons/ios`; apagamos essas pastas (app só desktop).
- O `target/` de um build debug do workspace ocupa ~4 GB; o disco tem pouca folga, então apague clones temporários depois dos checks.
- As libs apt vêm de `./scripts/setup-linux.sh` (já rodado neste runner).
