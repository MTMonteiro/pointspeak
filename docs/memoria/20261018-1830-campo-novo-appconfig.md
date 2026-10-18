# Campo novo no AppConfig: onde mexer

tipo: convenção
origem: task #815 · claude · 2026-10-18

- Núcleo: campo em `AppConfig` + `Default` em `crates/pointspeak-core/src/config.rs`; o teste `serializes_camel_case` compara o JSON inteiro e quebra de propósito.
- Fixture: preencha o campo em `crates/pointspeak-core/tests/contract_fixtures.rs` (de preferência com valor diferente do padrão) e regenere com o comando do AGENTS.md.
- TypeScript: tipo em `src/lib/contract.ts`, conferência em `isAppConfig` (`src/test/rust-contract.test.tsx`) e o `DEFAULT_CONFIG` de `src/settings/Settings.tsx` (tipado como `AppConfig`: sem o campo o `tsc`/build falha). Os mocks `CONFIG` de `Settings.test.tsx` e `SettingsModelsHardware.test.tsx` não são tipados, mas mantenha-os iguais ao contrato.
- `src-tauri/src/config.rs` não tem lista de campos: grava/lê o struct inteiro. Quem usa a config no ditado lê `current_config(&app)` na hora (`dictation.rs`), então um campo novo vale no próximo ditado sem mexer no `Input::ConfigChanged`.
- O primeiro `cargo clippy`/`cargo test -p pointspeak` de um worktree novo passa de 2 min: rode em background.
