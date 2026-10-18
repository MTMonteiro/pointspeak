# DESIGN.md — PointSpeak

Fonte da verdade do visual do PointSpeak: o HUD em pílula que segue o cursor e a janela de Configurações.
As telas React (`src/hud/HudOverlay.tsx`, `src/settings/Settings.tsx`, React 18 + Tailwind v4) seguem este
arquivo; os protótipos em `telas/` mostram cada estado. Índice e guia de implementação em [README.md](README.md).

## 1. Princípios

1. **Discreto.** O PointSpeak é um utilitário de bandeja. O HUD aparece só enquanto o usuário dita e some sozinho;
   nunca pede clique, nunca rouba foco (`focusable: false` + click-through).
2. **Ultraleve.** Fonte do sistema (sem webfont), sem imagens, ícones em SVG inline, animações só com `transform`
   e `opacity` (compostas na GPU, sem layout). O HUD não carrega nada além do próprio CSS.
3. **Estado legível de relance.** Cada estado tem uma cor e uma forma próprias (ponto vermelho + barras, spinner,
   check verde, ícone de alerta); a cor nunca é o único sinal: sempre há forma e texto.
4. **Local e honesto.** Os textos dizem o que acontece de verdade ("Transcrevendo...", "Modelo não encontrado") e
   lembram que o áudio não sai do computador.
5. **Acessível.** Contraste AA em todo texto (≥ 4,5:1) e em bordas de controles (≥ 3:1), foco visível, rótulos
   em todo controle, `prefers-reduced-motion` e `prefers-color-scheme` respeitados.

Textos da UI em **PT-BR**. Números no formato brasileiro (`1,6 GB`, `2,5 s`).

## 2. Tokens

Nomes pensados para virar variáveis CSS do Tailwind v4 (`@theme`): o token `color.surface` vira
`--color-surface` e a classe `bg-surface`. Modo claro é o padrão; o escuro entra por
`@media (prefers-color-scheme: dark)`. Contraste medido com a fórmula WCAG 2.1 (sobre a superfície indicada).

### 2.1 Cores

| Token | Claro | Escuro | Uso | Contraste (claro / escuro) |
| --- | --- | --- | --- | --- |
| `color.bg` | `#F6F7F9` | `#141518` | Fundo da janela de Configurações | — |
| `color.surface` | `#FFFFFF` | `#1C1E22` | Cartões, painel de conteúdo, pílula do HUD (sólida) | — |
| `color.surface-2` | `#EEF0F3` | `#25282D` | Hover, trilho de barras, campo inativo, esqueleto | — |
| `color.divider` | `#D0D7DE` | `#33373D` | Divisórias decorativas (não são controles) | — |
| `color.border-control` | `#7D8590` | `#737B85` | Borda de input, select, radio, checkbox | 3,6 / 3,9 sobre surface |
| `color.text` | `#1F2328` | `#F2F3F5` | Texto principal | 15,8 / 15,0 |
| `color.text-muted` | `#59636E` | `#A1A7B0` | Texto secundário, legendas | 6,1 / 6,9 |
| `color.text-disabled` | `#6E7781` | `#868E96` | Opção indisponível (ainda legível) | 4,6 / 5,0 |
| `color.accent` | `#4F46E5` | `#A5B4FC` | Ação primária, item ativo da navegação, foco | 6,3 / 8,4 |
| `color.on-accent` | `#FFFFFF` | `#1C1E22` | Texto sobre `accent` | 6,3 / 8,4 |
| `color.accent-soft` | `#EEF0FF` | `#262A45` | Fundo do item ativo, selo "Ativo" | accent sobre ele: 5,6 / 7,0 |
| `color.record` | `#D92D20` | `#FF6B5E` | Ponto pulsando e barras do HUD gravando | 4,8 / 6,0 |
| `color.success` | `#1A7F37` | `#3FB950` | Check do HUD, selo "Pronto", progresso concluído | 5,1 / 6,6 |
| `color.success-soft` | `#EDFAF1` | `#1B2B20` | Fundo de aviso de sucesso | success sobre ele: 4,7 / 5,9 |
| `color.danger` | `#B42318` | `#F97066` | Erro no HUD e nos formulários, botão remover | 6,6 / 6,0 |
| `color.danger-soft` | `#FEF3F2` | `#2A1C1C` | Fundo de aviso de erro | danger sobre ele: 6,1 / 5,9 |
| `color.warning` | `#9A6700` | `#E3B341` | Aviso de fallback para CPU, conflito de atalho | 4,5 sobre warning-soft / 8,6 |
| `color.warning-soft` | `#FFF8C5` | `#2B2410` | Fundo do aviso | 4,5 / 7,9 |
| `color.hud-bg` | `rgb(255 255 255 / 0.96)` | `rgb(28 30 34 / 0.94)` | Fundo da pílula (quase sólido: o texto mantém AA sobre qualquer app) | ≥ 15 / ≥ 14 |
| `color.hud-border` | `rgb(31 35 40 / 0.12)` | `rgb(255 255 255 / 0.10)` | Contorno de 1 px da pílula, separa do app de baixo | — |
| `color.focus` | = `accent` | = `accent` | Anel de foco | — |

Regra: componente usa token, nunca hex solto. O `color.record` só aparece no HUD gravando e no ícone da bandeja
gravando, para manter o vermelho = "microfone aberto".

### 2.2 Tipografia

Pilha do sistema, sem webfont: `font.sans = system-ui, -apple-system, "Segoe UI", Roboto, Ubuntu, Cantarell,
"Noto Sans", sans-serif`. Números que mudam (tempo, porcentagem, nível) usam `font-variant-numeric: tabular-nums`
para não tremer. Código/atalho: `font.mono = ui-monospace, "Cascadia Mono", "DejaVu Sans Mono", monospace`.

| Token | Tamanho / altura | Peso | Uso |
| --- | --- | --- | --- |
| `text.hud` | 13 px / 16 px | 500 | Texto da pílula |
| `text.caption` | 12 px / 16 px | 400 | Legendas, tamanho do modelo, motivo de indisponível |
| `text.body` | 14 px / 20 px | 400 | Texto padrão das Configurações |
| `text.label` | 14 px / 20 px | 600 | Rótulo de campo, nome do modelo |
| `text.title` | 18 px / 24 px | 600 | Título da seção |
| `text.kbd` | 12 px / 16 px | 600 mono | Teclas do atalho (`Alt` `Z`) |

### 2.3 Espaçamento, raio, sombra, tamanhos

Escala de espaço em múltiplos de 4: `space.1 = 4px`, `space.2 = 8px`, `space.3 = 12px`, `space.4 = 16px`,
`space.5 = 20px`, `space.6 = 24px`, `space.8 = 32px`. Tailwind v4 já usa `--spacing: 0.25rem`; basta manter.

| Token | Valor | Uso |
| --- | --- | --- |
| `radius.sm` | 6 px | Inputs, botões, selos |
| `radius.md` | 10 px | Cartões, linhas de modelo, avisos |
| `radius.lg` | 14 px | Painel do conteúdo |
| `radius.pill` | 999 px | Pílula do HUD, barras, chips, interruptor |
| `shadow.hud` | `0 1px 2px rgb(0 0 0 / .18), 0 4px 10px rgb(0 0 0 / .16)` | Sombra da pílula, **desenhada em CSS** (a janela tem `shadow: false`) |
| `shadow.card` | `0 1px 2px rgb(16 24 40 / .06)` | Cartões (claro); no escuro, sem sombra, só `divider` |
| `focus.ring` | `outline: 2px solid var(--color-focus); outline-offset: 2px` | Todo elemento focável (`:focus-visible`) |
| `size.hud-window` | 220 × 56 px | Janela `hud` (já está no `tauri.conf.json`; **não muda**) |
| `size.hud-pill-h` | 40 px | Altura da pílula |
| `size.hud-inset` | 8 px | Margem transparente da pílula até a borda da janela (espaço da sombra) |
| `size.hud-pill-max-w` | 204 px | Largura máxima da pílula (220 − 2 × 8) |
| `size.settings-window` | 720 × 560 px | Janela `settings` (já no `tauri.conf.json`); mínimo sugerido 360 × 480 |
| `size.target` | 32 px (mín. 24 px) | Alvo de clique de botões e controles |

### 2.4 Movimento

| Token | Valor | Uso |
| --- | --- | --- |
| `ease.standard` | `cubic-bezier(0.2, 0, 0, 1)` | Morph da pílula, entrada |
| `ease.exit` | `cubic-bezier(0.3, 0, 1, 1)` | Saídas e fade-out |
| `ease.inout` | `cubic-bezier(0.4, 0, 0.2, 1)` | Pulso do ponto |
| `dur.fast` | 120 ms | Troca de conteúdo (crossfade), hover |
| `dur.bar` | 90 ms | Transição de altura de cada barra (≥ 30 ms do evento, suaviza) |
| `dur.enter` | 160 ms | HUD aparecendo (opacity 0→1, scale .92→1) |
| `dur.morph` | 240 ms | Largura da pílula entre estados |
| `dur.success-fade` | **400 ms** | Fade-out do check verde |
| `dur.pulse` | 1200 ms | Ciclo do ponto vermelho |
| `dur.spin` | 800 ms | Volta do spinner |
| `dur.shimmer` | 1400 ms | Brilho passando no texto "Transcrevendo..." |

## 3. HUD (janela `hud`)

Protótipo: [`telas/hud.html`](telas/hud.html).

### 3.1 Janela e posição

- Janela 220 × 56 px lógicos, `transparent`, sem decoração, sem sombra nativa, `alwaysOnTop`, `skipTaskbar`,
  `focusable: false`, click-through (`set_ignore_cursor_events(true)`). O tamanho atual do `tauri.conf.json`
  serve; **a fase Telas não precisa mudar a janela**.
- `html, body` com `background: transparent` e `margin: 0`; a pílula é o único elemento visível.
- A pílula fica a 8 px (`size.hud-inset`) da borda superior e da esquerda da janela, para a sombra CSS caber.
- **X11 e Windows (segue o cursor):** a pílula deve aparecer a **+16 px, +16 px** do ponto do cursor. Como a
  pílula tem 8 px de margem dentro da janela, a janela vai em `cursor + (8, 8)`. Pílula alinhada à **esquerda**
  da janela (cresce para a direita, junto do cursor).
  - Perto da borda direita/inferior da tela: inverter o lado. Se `cursor.x + 8 + 220 > tela.direita`, janela em
    `x = cursor.x − 8 − 220` e a pílula alinha à direita; o mesmo para `y` com 56. Nunca sair da área de trabalho.
  - Atualização a ~60 Hz só enquanto o HUD está visível (spec, seção 3).
- **Wayland (posição fixa):** o Wayland não expõe a posição global do cursor. A janela fica no **centro-inferior
  da tela ativa**: `x = área.x + (área.largura − 220) / 2`, `y = área.y + área.altura − 56 − 48` (48 px acima da
  borda inferior da área de trabalho, acima de painéis/docas). A pílula fica **centralizada** na janela. Nada
  muda nos estados.
- Atributos para o React: `data-state="recording|transcribing|success|error"` e `data-anchor="start|end|center"`
  (esquerda, direita invertida, Wayland).

### 3.2 Anatomia da pílula

Altura 40 px, `radius.pill`, fundo `color.hud-bg`, contorno 1 px `color.hud-border`, `shadow.hud`, padding
horizontal 14 px, itens com gap de 8 px, `text.hud`. Largura = conteúdo (máx. 204 px; texto longo corta com
reticências e o texto inteiro vai no `aria-label`). O nó raiz tem `role="status"` e `aria-live="polite"`.

| Estado | Conteúdo (esquerda → direita) | Largura aprox. | Texto lido pelo leitor de tela |
| --- | --- | --- | --- |
| `idle` | **Oculto.** A janela fica escondida (`window.hide()`); o React renderiza `null`. Nada é desenhado. | — | — |
| `recording` | Ponto 8 px `color.record` pulsando · "Gravando" · 5 barras · tempo `0:04` (`text-muted`, tabular) | ~180 px | "Gravando" |
| `transcribing` | Spinner 14 px (traço 2 px `color.accent`) · "Transcrevendo..." com shimmer | ~160 px | "Transcrevendo" |
| `success` | Círculo 18 px `color.success` com check branco · "Texto colado" | ~137 px | "Texto colado" |
| `error` | Ícone de alerta 16 px `color.danger` · mensagem em `color.danger` | até 204 px | a mensagem |

**Barras do áudio** (`recording`): 5 barras de 3 × 20 px, gap 3 px, `radius.pill`, cor `color.record`, desenhadas
em altura máxima e escaladas com `transform: scaleY(s)` a partir do centro (`transform-origin: center`).

- Entrada: evento `hud://audio-level` com RMS normalizado `level` de 0 a 1, a cada ~30 ms.
- Curva: `v = clamp(sqrt(level), 0, 1)` (a raiz realça a fala baixa).
- Peso por barra (centro mais alto): `[0.55, 0.8, 1, 0.8, 0.55]`; a barra `i` usa `s = max(0.2, v × peso[i])`
  (0,2 = 4 px, a barra nunca some).
- Suavização: `transition: transform dur.bar linear`; opcionalmente ataque rápido e queda lenta
  (`s = s_novo > s_atual ? s_novo : s_atual × 0.85 + s_novo × 0.15`).
- Exemplos no protótipo: silêncio (0), fala baixa (0,15), fala normal (0,45), fala alta (0,9).

**Mensagens de erro** (curtas: no máximo ~22 caracteres para caber em 204 px; medido com DejaVu Sans, a fonte mais larga da pilha):

| Causa | Texto | Visível por |
| --- | --- | --- |
| Microfone não abre / foi desconectado | `Microfone indisponível` | 3000 ms + fade 400 ms |
| Modelo ativo não está no disco | `Modelo não encontrado` | 3000 ms + fade 400 ms |
| Inferência falhou | `Falha na transcrição` | 3000 ms + fade 400 ms |
| Colagem falhou (o texto fica no clipboard) | `Cole com Ctrl+V` (`aria-label`: "Não deu para colar; o texto está copiado, cole com Ctrl+V") | 4000 ms + fade 400 ms |
| Nenhuma fala detectada | `Nada foi ouvido` | 2000 ms + fade 400 ms |

Depois do tempo, o frontend dispara `Dismiss` (estado `idle`) e a janela esconde. O detalhe do erro fica no log e,
quando servir, na seção correspondente das Configurações.

### 3.3 Animações

| Animação | Como | Duração / easing |
| --- | --- | --- |
| Entrada (`idle → recording`) | `opacity 0→1`, `scale .92→1` (origem no canto do cursor) | `dur.enter`, `ease.standard` |
| Ponto pulsando | anel `box-shadow` do ponto cresce e some (`0 0 0 0 → 0 0 0 6px` transparente), ponto `opacity 1 → .7 → 1` | `dur.pulse`, `ease.inout`, infinito |
| Barras | `scaleY` por nível RMS | `dur.bar`, linear |
| Morph `recording → transcribing` | largura da pílula anima (medir com `ResizeObserver` ou largura fixa por estado: 180 → 150); conteúdo antigo `opacity 1→0` em 120 ms, novo `0→1` em 120 ms com 60 ms de atraso | `dur.morph`, `ease.standard` |
| Spinner | arco de 270° girando | `dur.spin`, linear, infinito |
| Shimmer do texto | `background-clip: text` com faixa clara passando sobre a cor `text` (contraste mínimo do texto continua AA) | `dur.shimmer`, linear, infinito |
| Sucesso | check entra com `scale .6→1` em 160 ms, fica 500 ms, **fade-out 400 ms** da pílula inteira; depois `hide()` | total 1060 ms; fade `ease.exit` |
| Erro | entra com morph normal; fica o tempo da tabela; fade-out 400 ms | `ease.exit` |

**`prefers-reduced-motion: reduce`:** sem pulso (ponto fixo), sem spinner girando (troca por três pontos
estáticos `•••` em `accent`), sem shimmer, sem morph (troca instantânea), sem scale na entrada (só opacity em
120 ms). As barras **continuam** mostrando o nível (é informação), mas em 3 degraus (baixo/médio/alto) e sem
transição. O sucesso fica 700 ms visível e some com fade de 120 ms.

### 3.4 Claro e escuro

A pílula segue o tema do sistema (`prefers-color-scheme`), não o app de baixo. Por isso o fundo é quase sólido
(96 %/94 %) com contorno de 1 px e sombra: legível sobre fundo claro, escuro ou "ocupado" (foto, código).
O protótipo mostra as duas variantes sobre os três fundos.

## 4. Janela de Configurações (janela `settings`)

Protótipo: [`telas/settings.html`](telas/settings.html).

### 4.1 Layout

- 720 × 560 px. Fundo `color.bg`. Barra lateral de 184 px (navegação) + área de conteúdo com rolagem vertical
  própria, padding 24 px. Título da seção em `text.title` + uma linha de descrição em `text-muted`.
- **Celular primeiro / janela estreita (< 600 px):** a barra lateral vira uma faixa de chips que quebra linha no
  topo (sem rolagem horizontal); o conteúdo ocupa a largura toda com padding 16 px. Funciona a partir de 360 px.
- Rodapé da barra lateral: **estado do motor** (ex.: `● Pronto · base · CPU`, `◌ Carregando modelo…`,
  `● Nenhum modelo ativo` em `warning`). É o único lugar que resume se o ditado vai funcionar.
- **Salvar automático:** cada mudança vale na hora e mostra o aviso efêmero `Salvo` (2 s) no topo do conteúdo.
  Não há botão "Salvar".
- Navegação: `<nav aria-label="Seções">` com links; item ativo com `aria-current="page"`, fundo `accent-soft`,
  texto `accent`. Setas ↑/↓ não são obrigatórias; Tab percorre os itens.

### 4.2 Seções

| Seção | Conteúdo | Estados |
| --- | --- | --- |
| **Microfone** | Select do dispositivo ("Padrão do sistema (…)" primeiro); botão `Testar microfone` que abre o medidor de nível (barra horizontal 8 px, `success` até 70 %, `warning` até 90 %, `danger` acima; texto "Fale algo…"/"Ótimo nível"/"Muito alto"); botão vira `Parar teste`. | carregando ("Procurando microfones…", esqueleto do select); vazio ("Nenhum microfone encontrado" + `Procurar de novo`); erro ("Sem permissão para usar o microfone" / "O dispositivo foi desconectado"). |
| **Modelos** | Lista de 4 modelos Whisper: `tiny` (75 MB, "Mais rápido, menos preciso"), `base` (142 MB, "Equilíbrio, recomendado"), `small` (466 MB, "Mais preciso, mais lento"), `large-v3-turbo` (1,6 GB, "O mais preciso; pede GPU/NPU"). Cada linha: nome, tamanho, descrição, selo de estado, ações. Rodapé: pasta dos modelos + espaço usado + `Abrir pasta`. | Por modelo: **não baixado** (`Baixar`); **baixando** (barra de progresso, `42 % · 196 de 466 MB`, `Cancelar`); **verificando SHA-256** (barra indeterminada, "Verificando integridade…"); **pronto** (selo `Pronto`, `Usar este`, `Remover`); **ativo** (selo `Ativo` em accent, `Remover` desabilitado com motivo "Troque o modelo ativo antes de remover"); **falha** (mensagem em `danger`: "Download falhou: conexão perdida" ou "Arquivo corrompido (SHA-256 não confere)" + `Tentar de novo`). Lista: carregando (esqueletos); vazio (nenhum baixado: aviso "Baixe um modelo para começar a ditar. Recomendado: base"); erro ("Não foi possível ler a pasta dos modelos"). Remover pede confirmação inline (`Remover 466 MB?` `Remover` / `Cancelar`). |
| **Hardware** | Radios em cartões: `NPU (DirectML)`, `OpenVINO (Intel)`, `Vulkan (GPU)`, `CUDA (NVIDIA)`, `CPU`. Cada um com a linha de detalhe do dispositivo detectado. `CPU` sempre disponível. | Indisponível: radio desabilitado, texto `text-disabled` e motivo ("Disponível só no Windows", "Nenhuma GPU NVIDIA encontrada", "Driver Vulkan não encontrado", "Nenhum dispositivo Intel compatível"). Carregando: "Detectando hardware…". **Aviso de fallback** (`warning`): "A aceleração Vulkan não iniciou. O PointSpeak está usando a CPU. Motivo: …" + `Tentar de novo`. |
| **Idioma** | Controle segmentado (radiogroup): `Português (PT-BR)` / `English (EN)` / `Automático`. Legenda: "No automático o modelo detecta o idioma a cada ditado; fixar o idioma é mais rápido e erra menos." | — (sempre disponível) |
| **Atalho** | Campo de captura mostrando as teclas (`Alt` + `Z`, padrão) + `Alterar`; radios do modo: `Segurar para falar (Push-to-Talk)` / `Tocar para começar e parar (Toggle)`; interruptor "Parar sozinho após 2,5 s de silêncio"; `Restaurar padrão`. | Capturando: campo com borda accent, "Pressione a nova combinação… (Esc cancela)". Conflito (`warning`): "Ctrl+Space já é usado por outro app ou pelo sistema. Escolha outra combinação." Inválido (`danger`): "Use ao menos um modificador (Ctrl, Alt, Shift ou Super) com uma tecla." Wayland: aviso informativo se o compositor não permitir atalho global. |
| **Sobre** | Ícone, `PointSpeak`, `Versão 0.1.0`, motor e modelo em uso; destaque: "**Seu áudio nunca sai deste computador.** A transcrição roda aqui mesmo; a internet só é usada para baixar modelos." Links: `Licenças`, `Abrir pasta de logs`. | — |

### 4.3 Componentes (e estados)

| Componente | Estados obrigatórios |
| --- | --- |
| `Button` (primário `accent`, secundário com borda, perigo `danger`, fantasma) | normal, hover (`surface-2`), foco (`focus.ring`), pressionado, desabilitado (`text-disabled`, `cursor: not-allowed`, motivo em `title` **e** em texto ao lado), carregando (spinner 14 px + texto "Baixando…") |
| `Select` / `Input` | normal (`border-control`), foco, erro (borda `danger` + mensagem ligada por `aria-describedby`), desabilitado, carregando (esqueleto) |
| `RadioCard` (Hardware) | normal, selecionado (borda 2 px `accent`, fundo `accent-soft`), desabilitado com motivo, foco |
| `Segmented` (Idioma) | normal, selecionado, foco |
| `Switch` | ligado (`accent`), desligado (`border-control`), foco, desabilitado; `role="switch"` + `aria-checked` |
| `Progress` | determinado (`role="progressbar"` com `aria-valuenow`), indeterminado (faixa deslizando; reduced-motion: barra listrada estática) |
| `LevelMeter` | parado, ativo (nível), sem sinal ("Nenhum som detectado — confira o microfone") |
| `ModelRow` | não baixado, baixando, verificando, pronto, ativo, falha, removendo (confirmação) |
| `Callout` | info (`accent-soft`), sucesso, aviso, erro; ícone + texto + ação opcional; `role="status"` (ou `alert` em erro) |
| `Badge` | Ativo, Pronto, Recomendado, Indisponível |
| `KeyCap` | tecla em `text.kbd`, fundo `surface-2`, borda `divider`, `radius.sm` |
| `Skeleton` | bloco `surface-2` com brilho (reduced-motion: estático) |

## 5. Menu da bandeja

O PointSpeak inicia minimizado na bandeja. Hoje o `src-tauri/src/tray.rs` tem `Configurações` e `Sair`; o
desenho completo é:

```
PointSpeak — Pronto (Alt+Z)        ← item desabilitado, só informa o estado
───────────────────────────
Pausar ditado                      ← vira "Retomar ditado" quando pausado
Configurações…
───────────────────────────
Sair
```

- **Pausar ditado:** desliga o atalho global (o HUD não aparece) até retomar; não descarrega o modelo.
- Linha de estado (texto do primeiro item e tooltip do ícone): `Pronto (Alt+Z)`, `Gravando…`, `Transcrevendo…`,
  `Pausado`, `Carregando modelo…`, `Nenhum modelo ativo — abra as Configurações`.
- Clique esquerdo no ícone (onde o sistema permite): abre as Configurações. Clique direito: menu.

**Ícone** (monocromático, 16/22/32 px, segue o tema do painel quando o SO deixar):

| Estado | Ícone |
| --- | --- |
| Pronto | glifo do PointSpeak (balão de fala com ponto) |
| Gravando | glifo + ponto `record` no canto inferior direito |
| Transcrevendo | glifo + ponto `accent` |
| Pausado | glifo com 50 % de opacidade e barra diagonal |
| Atenção (sem modelo, erro do motor) | glifo + ponto `warning` |

Os ícones finais são desenhados na fase de empacotamento (`src-tauri/icons/`); aqui fica só a regra.

## 6. Lacunas conhecidas

- Ícones da bandeja por estado ainda não existem como arquivo (só a regra acima).
- O HUD não mostra o texto transcrito (fora da spec); pode entrar depois como pré-visualização.
- Escala de DPI (Windows 125 %/150 %) usa px lógicos do Tauri; conferir a nitidez das barras de 3 px num Windows real.
- Wayland: se o compositor não aceitar janela sempre no topo/click-through, o HUD pode precisar de fallback
  (notificação do sistema); validar na fase de integração com o SO.
