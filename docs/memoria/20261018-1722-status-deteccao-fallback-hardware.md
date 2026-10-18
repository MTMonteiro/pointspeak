# Estado de detecção e fallback na seção Hardware

tipo: padrão
origem: task #795 · codex · 2026-10-18

- `list_backends` descreve disponibilidade detectada e detalhe do dispositivo; isso não explica falhas ao abrir a engine/modelo. A causa de execução deve vir do `fallbackReason` opcional em `EngineStatus` (`fallback_reason` no Rust, serializado como camelCase).
- Cubra o caso em que o cartão pedido continua disponível, mas o motor efetivo está em CPU: o aviso deve mostrar a causa do motor e o rodapé deve continuar mostrando o backend efetivo.
- A ação de tentar de novo atualiza a lista de backends e o estado atual do motor; ela não inicia uma nova transcrição.
