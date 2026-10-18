# docs/design — PointSpeak

Referência visual do PointSpeak, mantida pela persona `designer`. A fase Telas (React 18 + Tailwind v4, em
`src/hud/` e `src/settings/`) implementa o que está aqui.

| Arquivo | O que é | Obrigatório? |
| --- | --- | --- |
| [`DESIGN.md`](DESIGN.md) | Fonte da verdade: princípios, tokens, medidas do HUD, animações, seções das Configurações, componentes e estados, menu da bandeja | **Sim**: tokens, medidas, durações, textos e estados |
| [`telas/hud.html`](telas/hud.html) | Protótipo do HUD: estados lado a lado (claro/escuro × fundo claro/escuro/ocupado), demonstração animada, posição X11/Windows e Wayland | Visual e textos **sim**; o JS da página é só demonstração |
| [`telas/settings.html`](telas/settings.html) | Protótipo da janela de Configurações com as seis seções e seus estados | Layout, textos e estados **sim**; dados (nomes de dispositivos, GPUs) são exemplos |

Origem: tudo foi criado nesta pasta pela task #758 (fase 2, Design e UI), a partir de `docs/produto.md` (seção 3),
`src-tauri/tauri.conf.json` e `crates/pointspeak-core/src/hud.rs`. Não há pacote recebido (`recebido/`).

## Como abrir

Sem build e sem internet: abra o arquivo direto no navegador (`xdg-open docs/design/telas/hud.html` ou arraste
para a janela). Os arquivos são autocontidos (CSS e JS inline, ícones em SVG inline, fonte do sistema).
Se o navegador bloquear `file://`, sirva a pasta: `python3 -m http.server -d docs/design 8765` e abra
`http://127.0.0.1:8765/telas/hud.html`.

- `hud.html`: botões no topo trocam o estado da demonstração e simulam `prefers-reduced-motion`. O tema da
  página segue o sistema; as grades mostram os dois temas fixos.
- `settings.html`: a barra tracejada (só do protótipo) troca o **Tema** (sistema/claro/escuro), a **Largura**
  (720 px ou 360 px) e o **Estado** da seção aberta. O link `#modelos`, `#hardware` etc. abre direto a seção.
  Botões `Baixar` simulam baixando → verificando → pronto; `Testar microfone` anima o medidor.

## Mapa estado/seção → arquivo

| Estado ou seção | Onde ver | Regra no DESIGN.md |
| --- | --- | --- |
| HUD `idle` | (oculto; nota no topo de `hud.html`) | §3.2 |
| HUD `recording` (níveis 0 / 0,15 / 0,45 / 0,9) | `hud.html` → Estados lado a lado | §3.2 barras, §3.3 |
| HUD `transcribing` | `hud.html` → Estados lado a lado | §3.2, §3.3 morph/spinner/shimmer |
| HUD `success` (fade-out 400 ms) | `hud.html` → Estados e Demonstração | §3.3 |
| HUD `error` (Microfone indisponível, Modelo não encontrado, Cole com Ctrl+V) | `hud.html` → Estados | §3.2 tabela de mensagens |
| HUD reduced-motion | `hud.html` → Demonstração, "Simular reduced-motion" | §3.3 |
| Posição +16/+16, inversão na borda, Wayland centro-inferior | `hud.html` → Posição na tela | §3.1 |
| Microfone: normal (teste de nível), carregando, vazio, erro | `settings.html#microfone` + Estado | §4.2 |
| Modelos: não baixado, baixando, verificando SHA-256, pronto, ativo, falha, remover, carregando, vazio, erro | `settings.html#modelos` + Estado (Normal, Verificando e falha, Remover, Carregando, Vazio, Erro) | §4.2 |
| Hardware: Linux (indisponíveis com motivo), Windows, fallback para CPU, carregando | `settings.html#hardware` + Estado | §4.2 |
| Idioma PT-BR / EN / Automático | `settings.html#idioma` | §4.2 |
| Atalho: normal (Alt+Z), capturando, conflito, inválido, Wayland; Push-to-Talk/Toggle | `settings.html#atalho` + Estado | §4.2 |
| Sobre (versão, áudio local) | `settings.html#sobre` | §4.2 |
| Janela estreita (360 px) | `settings.html`, Largura 360 px | §4.1 |
| Menu e ícone da bandeja | só texto | §5 |

## Fluxo

Atalho → HUD `recording` (pílula aparece junto do cursor; no Wayland, no centro-inferior) → solta/toca de novo
ou 2,5 s de silêncio → `transcribing` → `success` (texto colado, some em 400 ms) ou `error` (some após 2–4 s).
As Configurações abrem pela bandeja (`Configurações…`) e salvam cada mudança na hora.

## Como implementar em React/Tailwind v4

1. **Tokens → `@theme`** em `src/styles.css` (as tabelas do DESIGN.md §2 já usam esses nomes):

   ```css
   @import "tailwindcss";
   @theme {
     --color-bg: #F6F7F9; --color-surface: #FFFFFF; --color-surface-2: #EEF0F3; --color-divider: #D0D7DE;
     --color-border-control: #7D8590; --color-text: #1F2328; --color-text-muted: #59636E;
     --color-text-disabled: #6E7781; --color-accent: #4F46E5; --color-on-accent: #FFFFFF;
     --color-accent-soft: #EEF0FF; --color-record: #D92D20; --color-success: #1A7F37;
     --color-success-soft: #EDFAF1; --color-danger: #B42318; --color-danger-soft: #FEF3F2;
     --color-warning: #9A6700; --color-warning-soft: #FFF8C5;
     --color-hud-bg: rgb(255 255 255 / 0.96); --color-hud-border: rgb(31 35 40 / 0.12);
     --font-sans: system-ui, -apple-system, "Segoe UI", Roboto, Ubuntu, Cantarell, "Noto Sans", sans-serif;
     --radius-sm: 6px; --radius-md: 10px; --radius-lg: 14px;
     --shadow-hud: 0 1px 2px rgb(0 0 0 / .18), 0 4px 10px rgb(0 0 0 / .16);
     --ease-standard: cubic-bezier(0.2, 0, 0, 1); --ease-exit: cubic-bezier(0.3, 0, 1, 1);
   }
   @media (prefers-color-scheme: dark) {
     :root { --color-bg: #141518; --color-surface: #1C1E22; /* … resto da coluna "Escuro" */ }
   }
   ```

   Com isso saem classes como `bg-surface`, `text-text-muted`, `border-border-control`, `rounded-md`,
   `shadow-hud`, `ease-standard`. Durações (`dur.*`) podem ficar como variáveis comuns (`--dur-morph`) usadas em
   `duration-(--dur-morph)` ou no CSS dos keyframes.
2. **HUD** (`src/hud/`): `html, body { background: transparent }`. Componentes: `HudPill` (contêiner com
   `role="status"`, `data-state`, `data-anchor`), `PulseDot`, `LevelBars` (recebe `level` 0..1 do
   `hud://audio-level`; aplica a curva e os pesos do §3.2 com `transform: scaleY`), `Spinner`, `SuccessCheck`,
   `ErrorIcon`. Keyframes (`pulse`, `spin`, `shimmer`, `enter`, `leave`) podem ser copiados de `telas/hud.html`.
   No `idle` retorne `null`. Timers de sucesso/erro chamam o `Dismiss`.
3. **Configurações** (`src/settings/`): `SettingsLayout` (sidebar + conteúdo, chips abaixo de 600 px; use
   container query ou `@media`), `NavItem`, `EngineStatus`, e um componente por seção (`MicrophoneSection`,
   `ModelsSection`, `HardwareSection`, `LanguageSection`, `ShortcutSection`, `AboutSection`). Componentes base:
   `Button`, `Select`, `RadioCard`, `Segmented`, `Switch`, `Progress`, `LevelMeter`, `ModelRow`, `Callout`,
   `Badge`, `KeyCap`, `Skeleton`, `SavedToast`. Estados de cada um no DESIGN.md §4.3.
4. **Acessibilidade**: `:focus-visible` com o anel de 2 px; todo input com `<label>`; motivo de opção
   desabilitada em texto visível ligado por `aria-describedby`; progresso com `role="progressbar"`.
5. **Sem dependências novas**: ícones são SVG inline (copie dos protótipos), sem biblioteca de ícones nem webfont.
