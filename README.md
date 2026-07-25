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
- [`rtk`](https://github.com/rtk-ai/rtk) (opcional) — comprime o output que entra no
  contexto do mestre. Ver [Economia de tokens](#economia-de-tokens-rtk).

## Instalação

```bash
git clone https://github.com/Rafatakitani/rege.git
cd rege
cargo install --path .
```

Isso instala o binário `rege` em `~/.cargo/bin` (garanta que está no seu `PATH`).

Pra atualizar depois, de qualquer diretório:

```bash
rege update            # puxa e reinstala a última versão do upstream
rege update --branch x # ou uma branch/tag específica
rege update --verbose  # com o output cru do cargo (default é silencioso)
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
(sessões anteriores) · `/agents` (roster: conecta/remove CLIs, grava no config;
`/agents ativos` lista os workers rodando) · `/buddy` (bicho de estimação animado) · `/quit`
Digitar `/` abre um autocomplete com os comandos: `↑↓` navega, `Tab` completa.
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

## Economia de tokens (`rtk`)

[`rtk`](https://github.com/rtk-ai/rtk) é um proxy de CLI que filtra o output de comandos
antes dele virar contexto de LLM (-60% a -90% de tokens). O `rege` usa se estiver no
`PATH`, sem configuração:

```bash
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
rtk init -g            # hook do Claude Code (workers também ganham)
rtk init -g --opencode
rtk init -g --gemini --auto-patch
rtk init -g --codex
```

O que o `rege` roteia por `rtk`:

| caminho | vira | por quê |
|---------|------|---------|
| `diff_agent` / `review` (diff da branch do worker) | `rtk git diff` | é o maior bloco que entra no contexto do mestre |
| output dos workers (`git status`, testes, `ls`…) | hook do próprio CLI | `rtk init -g` reescreve os comandos Bash deles |

O que **fica cru** de propósito: o `.patch` do `open_pr` (diff condensado não aplica) e
o git plumbing interno (`rev-parse`, `worktree`, `commit`) — ninguém lê aquilo.

Pra comprimir também o `run_tests`, ponha o `rtk` no seu comando de verify:

```yaml
verify:
  command: rtk cargo test   # em vez de: cargo test
```

Quem manda, do mais específico pro mais geral: **`REGE_RTK`** > **`config.yml`** >
**autodetecção no `PATH`**.

```yaml
rtk:
  enabled: true         # ausente = auto (usa se o binário estiver no PATH)
  hook_workers: false   # instala o hook do rtk dentro do worktree de cada worker
  clis: [claude]        # quais workers recebem o hook
  init_args: [init, --hook-only]
```

`REGE_RTK=0 rege …` desliga numa execução só; `REGE_RTK=1` força mesmo sem detectar
o binário.

`hook_workers` é opt-in explícito e nunca liga por autodetecção: escrever arquivo de
hook dentro do worktree alheio é intrusivo demais pra acontecer sozinho. Comprimir o
diff que o rege já ia mostrar é passivo, então esse pode ser automático.

## Para IAs / agentes

Se você é um agente de IA (Claude, Codex, Gemini…) operando este repo ou usando o
`rege` como ferramenta, leia [`AGENTS.md`](AGENTS.md) — ele diz como invocar o `rege`
de forma headless, quais ferramentas MCP existem, e as regras (workers isolados em
worktree, nunca fazer merge sozinho, sempre abrir PR).

## Desenvolvimento

```bash
cargo test    # 123 testes
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
