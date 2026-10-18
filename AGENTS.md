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
- Nenhum teste pode depender de tela, microfone ou rede; lógica pura vai em `crates/pointspeak-core`.
- Design e UI: o visual segue `docs/design/` (DESIGN.md e telas); a persona `designer` mantém essa pasta.
- Mudou o plano, a stack, os comandos ou a organização das pastas: atualize `docs/` e este arquivo no mesmo PR.
- Nunca versione segredos (`.env`, chaves, tokens); use variáveis de ambiente.
<!-- fabrica:fim -->
