# Conferir os protótipos de docs/design no navegador

tipo: comando
origem: task #758 · claude · 2026-10-18

- O Playwright MCP bloqueia `file://`. Sirva a pasta: `python3 -m http.server -d docs/design 8765 --bind 127.0.0.1` e abra `http://127.0.0.1:8765/telas/hud.html`.
- Os screenshots do Playwright MCP só podem ser gravados dentro do worktree (`.playwright-mcp/`, que não está no `.gitignore`): apague a pasta antes de commitar.
- Checagem de rolagem horizontal em 360 px: `document.documentElement.scrollWidth > innerWidth` e, na janela de Configurações, `#content.scrollWidth > #content.clientWidth` para cada estado (caminhos longos em `<code>` estouram sem `overflow-wrap: anywhere`).
- Nos protótipos, blocos de estado usam `data-v` + `display: revert`; elemento que precisa ser flex leva a classe `flexv` (estilo inline `display:flex` venceria o `display:none`).
