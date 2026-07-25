# AGENTS.md

Instruções para IAs / agentes que operam este repo ou usam o `rege` como ferramenta.
(Convenção `AGENTS.md` — lida por Codex, Claude Code e afins.)

## O que é o `rege`

Orquestrador multi-agente: um **mestre** (modelo principal) comanda outros CLIs de IA
como **workers**, cada um isolado num `git worktree` + sessão `tmux`. O mestre triagem
a dificuldade, delega, revisa e **abre um PR** — nunca faz merge sozinho.

## Como você (IA) invoca o `rege`

**Headless, uma tarefa (tipo `codex exec`):**
```bash
rege exec "descreva a tarefa aqui"
```
Isso já roda o mestre com o playbook + servidor MCP: ele pode spawnar workers, esperar,
revisar e abrir PR sozinho. Use pra delegar uma tarefa inteira.

**Interativo, no modo mestre:**
```bash
rege claude   # abre o claude já como orquestrador (playbook + MCP + auto-aprovação)
```

**Só o servidor MCP (pra plugar num cliente MCP seu):**
```bash
rege mcp-serve --repo /caminho/do/repo
```
Fala JSON-RPC 2.0 newline-delimited sobre stdio: `initialize`, `tools/list`, `tools/call`.

**Inspecionar a TUI sem terminal (útil em CI/headless):**
```bash
rege render --demo
```

## Ferramentas MCP disponíveis

Quando você é o **mestre**, tem estas ferramentas (via MCP):

| ferramenta | o que faz |
|------------|-----------|
| `spawn_agent` | dispara um worker (CLI de IA) isolado num git worktree pra uma tarefa |
| `list_agents` | lista os agentes e seus estados |
| `agent_status` | estado atual de um agente |
| `wait_agent` | **bloqueia até o agente terminar** (ou timeout) e commita o trabalho — use após `spawn_agent` |
| `read_output` | saída acumulada de um agente |
| `send_message` | injeta texto na sessão de um agente (redirecionar / cochichar / takeover) |
| `kill_agent` | mata a sessão de um agente |
| `diff_agent` | diff da branch de um agente |
| `review` | monta o contexto de revisão: diffs das branches dos agentes dados |
| `run_tests` | roda o comando de verify no worktree do agente (se configurado) |
| `consult` | pergunta pontual a um modelo mais forte (ex: opus) sem spawnar worker — escalação de raciocínio |
| `open_pr` | abre um PR a partir de uma branch (**nunca faz merge**); fallback: patch local |

## Regras (não negocie)

1. **Workers são isolados** em `git worktree` — a branch atual nunca é tocada direto.
2. **Sempre espere** o worker (`wait_agent`) antes de revisar; senão o `-p` sai antes.
3. **Nunca faça merge sozinho.** A saída final é sempre um **PR** (`open_pr`) pra
   aprovação humana — às vezes precisa de review de outras pessoas.
4. **Triagem:** fácil = dividir & conquistar (workers em partes diferentes → merge →
   revisão). Difícil = redundância & juiz (vários fazem o mesmo → merge sintético →
   loop de conserto, máx 3 rodadas, roda os testes se existirem).
5. **Escale com parcimônia:** default barato (ex: sonnet); use `consult`/opus só quando
   a decisão for difícil.
6. Se travar, **mate e siga** com os que terminaram (timeout por worker).

## Trabalhando NESTE repositório (rege em si)

- Rust. `cargo test` (164 testes) deve passar antes de commitar. `cargo fmt` + `cargo clippy`.
- **Versão**: `version` no `Cargo.toml` sobe em todo PR que muda comportamento —
  senão `rege --version` não distingue builds e ninguém sabe se o `update` pegou.
  O `build.rs` estampa o commit por cima (`rege 0.2.0 (b178154)`); o hash é
  automático, o semver é manual.
- **Overlays da TUI limpam com `Clear`**, nunca com `Paragraph::new("")` — o
  Paragraph vazio não desenha nada e deixa o que está embaixo aparecendo através
  do painel. Já foi bug em todos os cinco overlays de uma vez.
- Módulos em `src/`: `config`, `command`, `worktree`, `tmux`, `agent`, `engine`,
  `session`, `mcp`, `theme`, `tui`, `buddy`, `stream`, `driver`, `sessions`, `playbook`,
  `rtk`, `scan`.
- **`rtk`**: se o binário [`rtk`](https://github.com/rtk-ai/rtk) estiver no `PATH`, o
  diff que vai pro contexto do mestre (`diff_agent`/`review`) passa por `rtk git diff`
  (-75% de tokens). O `.patch` do `open_pr` e o git plumbing continuam crus — diff
  condensado não aplica. Regra ao mexer aqui: **só comprima o que um LLM vai ler**; o
  que é consumido por máquina fica cru.
  Precedência num lugar só (`rtk::resolve`): `REGE_RTK` > `config.yml` >
  autodetecção no `PATH`. Não crie um segundo botão pra mesma decisão.
  `rtk.hook_workers` (opt-in explícito) roda `rtk init --hook-only` dentro do worktree
  de cada worker listado em `rtk.clis` — autodetecção nunca liga isso sozinha.
- **`transcript`**: `/resume` repinta a conversa passada lendo o JSONL que o
  próprio `claude` guarda em `~/.claude/projects/<slug>/<id>.jsonl`. É
  best-effort e só vale pro `claude` — outro CLI (ou histórico limpo) retoma sem
  replay, como antes. Não inventamos formato de transcript: se a origem não
  existe, não há o que mostrar.
- **`scan`**: primeira vez num diretório → oferece escrever um `AGENTS.md` descrevendo
  ele. Coleta determinística e limitada (`MAX_FILES`/`MAX_DEPTH`) + uma chamada ao
  mestre com o digest pronto. A resposta fica em `~/.config/rege/scanned.yml`, nunca no
  projeto do usuário; nunca sobrescreve um `AGENTS.md` existente sem `--force`.
- Desenho completo: `docs/superpowers/specs/2026-07-23-rege-design.md`.
- Preserve a atribuição MIT do `/buddy` (port de ramarivera/claude-buddy) no topo de `src/buddy.rs`.
