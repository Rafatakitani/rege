# Rege

> ⚠️ Nome provisório · projeto em desenvolvimento.

Orquestrador multi-agente de IAs, em Rust. Uma TUI de terminal onde você conversa
com um **mestre** (modelo principal, default `claude` — trocável) que **comanda
outros CLIs de IA** (`claude`, `codex`, `gemini`, `opencode`) como workers. O mestre
avalia a dificuldade da tarefa, monta um time, deixa cada worker trabalhar **isolado
num `git worktree` + sessão `tmux`**, revisa o resultado e **abre um PR** — nunca faz
merge sozinho.

```
você ⇄ TUI do mestre  →  mestre (claude/…) via MCP  →  Rege
                                                          │
                        ┌──────────────┬─────────────┬────┘
                  worker (worktree A)  (worktree B)  (worktree C)  → review → PR
```

O binário é, num só processo: **servidor MCP** (expõe as ferramentas ao mestre),
**controlador tmux/worktree** (spawna, monitora e injeta nos workers) e a **TUI**.

## ⚠️ Aviso importante

Os workers rodam **auto-aprovando** (`--dangerously-skip-permissions` / `--yolo` /
sandbox do codex): editam arquivos, rodam comandos e commitam **sem pedir permissão**.
Ficam confinados a um `git worktree` — a sua branch atual nunca é tocada direto, e a
saída final é sempre um **PR pra aprovação humana**. Ainda assim: **rode só em repos
que você controla e entende o que está sendo pedido.** Não é uma sandbox de segurança.

## Requisitos

- Rust / Cargo
- `git`, `tmux`
- Pelo menos um CLI de IA instalado e autenticado. **Hoje o mestre só está 100% wired
  para `claude`**; `codex`/`gemini`/`opencode` funcionam como workers em best-effort.
- `gh` (opcional) autenticado, pra abrir PRs — sem ele, cai pra `.patch`.

## Instalação

```bash
git clone <este-repo> rege && cd rege
cargo install --path .
```

## Uso

```bash
rege                       # abre a TUI (orquestrador com chat, agentes, temas)
rege exec "corrige o bug de login"   # headless (tipo `codex exec`), orquestra e imprime
rege claude                # abre o claude interativo já em modo Rege (playbook+MCP)
rege doctor                # health check do roster de CLIs + mestre atual
rege config                # imprime a config efetiva
rege mcp-serve --repo .    # servidor MCP puro (JSON-RPC stdio) pro repo
rege render --demo         # desenha um frame da TUI como texto (headless, sem tty)
```

### Comandos da TUI

`/help` · `/theme` (seletor com preview) · `/model <nome>` · `/config` · `/resume`
(sessões anteriores) · `/agents` · `/buddy` (bicho de estimação animado) · `/quit`
(ou `exit`). Selecionar texto com o mouse copia via OSC52 (funciona por `ssh`/`tmux`
com passthrough); desliga em `ui.auto_copy`.

**Remoto:** a TUI roda em terminal, então do celular/outro device basta `ssh` (via
Tailscale, p.ex.) + `tmux attach`. Sem app.

## Configuração

Camadas, deep-merge nesta ordem: defaults ← `~/.config/rege/config.yml` (global) ←
`.rege.yml` (por projeto). Ajustável: mestre (`master.cli` / `master.model`), roster
(papel→CLI→modelo), tema, `ui.auto_copy`, e mais.

```yaml
# ~/.config/rege/config.yml
master:
  cli: claude
  model: sonnet   # escala pra opus nos passos difíceis (planner/reviewer/consult)
ui:
  theme: hacker
  auto_copy: true
```

## Os dois modos de orquestração

- **Fácil** — dividir & conquistar: workers pegam partes diferentes → merge-tudo → revisão.
- **Difícil** — redundância & juiz: vários fazem o mesmo → merge sintético → loop de
  conserto (caça-bug, roda os testes se existirem, máx 3 rodadas).

## Desenvolvimento

```bash
cargo test    # 104 testes
cargo fmt && cargo clippy
```

A maior parte da versão Rust foi construída pelo próprio Rege (dogfooding): workers
em worktrees separados escreveram os módulos de backend. A implementação Ruby original
está preservada na branch `legacy-ruby`. Desenho completo em
`docs/superpowers/specs/2026-07-23-rege-design.md`.

## Créditos & licença

- MIT — veja [`LICENSE`](LICENSE).
- O `/buddy` é um port de [ramarivera/claude-buddy](https://github.com/ramarivera/claude-buddy)
  (MIT, © ramarivera): espécies, stats e algoritmo de geração determinística.
