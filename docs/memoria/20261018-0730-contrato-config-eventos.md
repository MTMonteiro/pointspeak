# Contrato de eventos e config: o que o frontend ainda não segue

tipo: armadilha
origem: task #762 · claude · 2026-10-18

- O payload de `hud://state-changed` agora é objeto `{ state, message? }` (`pointspeak-core/src/events.rs`). O placeholder `src/hud/HudOverlay.tsx` ainda faz `listen<HudState>` com string pura: a fase Telas precisa ler `event.payload.state`.
- Desde a task #807 o `build.rs` declara `AppManifest::commands`: comando novo do app entra na lista `COMMANDS` do `build.rs` e ganha `allow-<comando>` em `capabilities/settings.json` (o teste `crates/pointspeak-core/tests/capabilities.rs` cobra as duas coisas).
- `set_config` recebe `{ config: AppConfig }` no `invoke` (nome do parâmetro) e devolve a config gravada ou uma string de erro em PT-BR.
- Valor de enum desconhecido no `config.json` (ex.: `"language": "fr"`) invalida o arquivo inteiro e carrega os padrões; campo numérico/atalho fora da faixa só volta ao padrão sozinho (`AppConfig::sanitized`).
