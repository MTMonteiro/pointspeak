# Contrato de eventos e config: o que o frontend ainda não segue

tipo: armadilha
origem: task #762 · claude · 2026-10-18

- O payload de `hud://state-changed` agora é objeto `{ state, message? }` (`pointspeak-core/src/events.rs`). O placeholder `src/hud/HudOverlay.tsx` ainda faz `listen<HudState>` com string pura: a fase Telas precisa ler `event.payload.state`.
- Comandos próprios do app (`get_config`, `set_config`) não precisam de permissão em `capabilities/default.json` no Tauri v2 enquanto o `build.rs` não declarar `AppManifest::commands`. Se declarar, liste-os lá.
- `set_config` recebe `{ config: AppConfig }` no `invoke` (nome do parâmetro) e devolve a config gravada ou uma string de erro em PT-BR.
- Valor de enum desconhecido no `config.json` (ex.: `"language": "fr"`) invalida o arquivo inteiro e carrega os padrões; campo numérico/atalho fora da faixa só volta ao padrão sozinho (`AppConfig::sanitized`).
