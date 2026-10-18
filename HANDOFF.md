# Handover — Task 780: HUD animado

## Objetivo
Implementar `HudOverlay.tsx` como pílula animada orientada pelos eventos existentes do backend, conforme `docs/design/DESIGN.md` §2–3 e `docs/design/telas/hud.html`.

## Feito
- HUD com `idle` oculto, gravação com cronômetro/barras, transcrição, sucesso e cinco mensagens de erro.
- Contrato tipado dos eventos em `src/lib/contract.ts`; temas claro/escuro em `src/styles.css`; animações isoladas em `src/hud/hud.css`.
- Testes com payloads Tauri simulados e fake timers; documentação de estrutura e validação visual atualizada.
- Nenhuma dependência, janela Tauri, código Rust ou arquivo de Configurações foi alterado.

## Decisões
- A janela Rust continua responsável por dispensar o HUD. A interface só inicia o fade: sucesso dentro dos 400 ms e erro após `error_visible` mais o fade de 400 ms.
- Reduced motion mantém o HUD dentro do tempo de 400 ms do backend: sucesso espera 280 ms e faz fade em 120 ms; barras usam três níveis discretos sem transição.
- O worktree não tem remoto `origin`; `git fetch origin` não é possível. O `HEAD` e `main` locais estavam no mesmo commit `681942d` (distância local 0).

## Validação
- `npm ci`: concluído, 254 pacotes instalados, nenhuma vulnerabilidade reportada.
- `RUNNER_TEST_MAX_WORKERS=2 npx vitest run src/hud/HudOverlay.test.tsx`: 24 testes passaram.
- `npm run lint` e `npm run build` (inclui `tsc --noEmit`): passaram.
- Chromium no preview local com ponte Tauri simulada: sem overflow em 220, 360, 390 e 1280 px; gravação 184 × 40 px, transcrição central 172 × 40 px e erro alinhado ao fim dentro da janela 220 × 56 px. Isso verifica o preview, não a janela Tauri integrada.
- A suíte inteira e os checks Cargo são executados pelo `run_checks` do runner depois de `worker_done`, conforme o Ambiente da task.

## Arquivos
- `src/hud/HudOverlay.tsx`, `src/hud/hud.css`, `src/hud/HudOverlay.test.tsx`
- `src/lib/contract.ts`, `src/styles.css`
- `docs/estrutura.md`, `docs/checklist-manual.md`
- `.runner/revisao.md` é a ficha da task e fica fora do commit.

## Falta / limites
- A validação manual em Tauri real (atalho, microfone, click-through, DPI e Wayland) permanece no `docs/checklist-manual.md`.
- A suíte completa e os checks Rust ainda dependem da execução automática do runner.
