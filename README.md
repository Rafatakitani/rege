# Regente

> Nome provisório. Orquestrador multi-agente de IAs em Ruby.

Uma ferramenta de terminal onde você conversa com um **mestre** (modelo principal,
default Claude — trocável) que **comanda outros CLIs de IA** (`claude`, `codex`,
`gemini`, `opencode`). O mestre avalia a dificuldade da tarefa, monta um time, deixa
cada worker trabalhar **isolado num `git worktree` + sessão `tmux`**, revisa o
resultado e **abre um PR** — nunca faz merge sozinho.

## Como funciona

```
você ⇄ TUI do mestre (em tmux)  →  mestre (claude/…) via MCP  →  Regente (Ruby)
                                                                    │
                              ┌──────────────┬──────────────┬───────┘
                        worker (worktree A)  (worktree B)  (worktree C)   → review → PR
```

O app Ruby é, num só processo: **servidor MCP** (expõe as ferramentas ao mestre),
**controlador tmux** (spawna/monitora/injeta nos workers) e **CLI**.

## Instalação

```bash
cd ~/regente
bundle install
```

## Uso

```bash
regente "corrige o bug de login"   # comanda o mestre (abre em tmux persistente)
regente attach                     # entra na sessão do mestre (local ou via ssh)
regente doctor                     # health check dos bots do roster + config
regente config show                # ver config efetiva
regente config set master.cli gemini   # trocar o mestre
regente config set timeouts.worker 120
```

**Remote control:** a sessão do mestre roda em tmux (`regente-master`). Do celular/
outro device: `ssh` (via Tailscale) + `regente attach` → cai na TUI ao vivo.

## Configuração (MUITO configurável)

Camadas: `~/.config/regente/config.yml` (global) ← `.regente.yml` (por projeto).
Editável por arquivo ou por `regente config set`. Ajustável: mestre, roster
(papel→CLI→modelo), timeouts por papel, playbooks (rodadas de revisão), provider de
PR, sandbox.

## Os dois modos

- **Fácil** — dividir & conquistar: workers pegam partes diferentes → merge-tudo → revisão.
- **Difícil** — redundância & juiz: todos fazem o mesmo → merge sintético → loop de
  conserto (com caça-bug Fable, `verify` se existir, máx N rodadas).

## Segurança

Workers rodam auto-aprovando (YOLO), confinados ao worktree — a `main` nunca é tocada
direto; a saída é sempre um PR pra aprovação humana.

## Desenvolvimento

```bash
bundle exec rake test   # suíte minitest
```

Desenho completo: `docs/superpowers/specs/2026-07-23-regente-design.md`.
