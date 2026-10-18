# Fixtures Rust↔TypeScript e testes de integração

tipo: armadilha
origem: task #805 · codex · 2026-10-18

- As fixtures de contrato ficam em `src/test/fixtures/rust-contracts.json`; o teste Rust normal compara o JSON versionado com serde. Para regenerar, rode o teste ignorado `cargo test -p pointspeak-core --test contract_fixtures write_rust_contract_fixtures -- --ignored` e revise o diff.
- `ModelSection` atualiza a linha pelo evento `models://progress`, mas estados finais como `failed` recarregam `list_models`. Em testes integrados, faça o mock do catálogo refletir o estado persistido antes de verificar várias linhas juntas.
