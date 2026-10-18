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
- Comandos de verificação (precisam passar antes de entregar):
- Ainda não definidos: o PM da Fábrica define no primeiro turno de fase; registre aqui quando existirem.
- Design e UI: o visual segue `docs/design/` (DESIGN.md e telas); a persona `designer` mantém essa pasta.
- Mudou o plano, a stack, os comandos ou a organização das pastas: atualize `docs/` e este arquivo no mesmo PR.
- Nunca versione segredos (`.env`, chaves, tokens); use variáveis de ambiente.
<!-- fabrica:fim -->
