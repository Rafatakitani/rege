# Regente — orquestrador multi-agente de IAs (desenho)

> Nome provisório (placeholder — trocar depois). Data: 2026-07-23.

## Visão

Uma **TUI** (estilo Claude Code) onde o usuário conversa com um **mestre** (modelo
principal, default Claude, trocável). O mestre **comanda os outros modelos de IA**:
avalia a dificuldade da tarefa, monta um time sob medida, distribui o trabalho, deixa
cada agente trabalhar isolado num `git worktree`, revisa tudo no fim e **abre um PR**
para aprovação humana. Nunca faz merge sozinho.

Princípio central: **MUITO configurável** — quase tudo ajustável dentro da própria TUI
(escreve de volta em YAML em camadas). Sem MVP: constrói o sistema inteiro.

## Conceitos (ubiquitous language)

- **Mestre (master)** — o modelo com quem o usuário conversa; orquestra os demais. Trocável.
- **Worker** — agente de IA executando uma tarefa, isolado num worktree.
- **Roster** — mapa configurável `papel → CLI → modelo`.
- **Playbook** — receita pronta de orquestração (ex: `fácil`/`dividir`, `difícil`/`redundante`).
- **Run** — uma execução: da tarefa até o PR.
- **Triagem (triage)** — classificação de dificuldade que decide o playbook.
- **Fan-out / fan-in** — espalhar trabalho em N workers / juntar os resultados.
- **Takeover** — assumir um worker ao vivo e digitar direto na sessão dele.

## Arquitetura

```
                         ┌─────────────────────────────────────┐
   usuário ⇄ TUI  ⇄  MESTRE (claude/gemini/codex/opencode)      │
   (local ou                     │  usa tools via MCP           │
    remoto via                   ▼                              │
    tmux/ssh)          ┌──────────────────────┐                 │
                       │  Regente MCP server   │  (processo Ruby)│
                       │  (Ruby)               │                 │
                       └──────────┬───────────┘                 │
                                  │ spawn/read/send/review/pr    │
              ┌───────────────────┼───────────────────┐         │
              ▼                   ▼                   ▼          │
        [worker: tmux]      [worker: tmux]      [worker: tmux]   │
        git worktree A      git worktree B      git worktree C   │
        (claude/codex/…)    (…)                 (…)              │
              └─────── fan-in: revisor Opus + Fable ─────────────┘
                                  │
                                  ▼
                            abre PR (gh)
```

O **app Ruby** cumpre 3 papéis num processo: **servidor MCP** (expõe as tools ao mestre),
**controlador tmux** (spawna/monitora/injeta nos workers) e **TUI** (chat + config + painéis).

### Substrato de execução: tmux + git worktree

- Cada worker roda numa **sessão tmux** dedicada, dentro de um **git worktree** próprio
  (branch `regente/<run>/<agente>`). Isolamento nativo, zero corrida de arquivo.
- tmux dá de graça: PTY, persistência (sobrevive à queda do app/ssh), attach ao vivo.
- **Painel ao vivo local** = `tmux attach` na sessão do worker.
- **Takeover remoto** = stream do painel (`pipe-pane`/`capture-pane`) + injeção de teclas
  (`send-keys`) pela TUI.
- Fallback sem git: cai para execução de **1 agente só** (sem worktree).

### O mestre e as ferramentas (MCP)

O mestre é um CLI headless (`claude -p`, `gemini -p`, `codex`, `opencode run`) que conecta
ao **Regente MCP server** e comanda tudo via tool-calling nativo. Trocar o mestre =
apontar outro CLI para o mesmo servidor (config). Todos os 4 CLIs suportam MCP (verificado).

Tools expostas (esboço, evolui no plano):

| Tool | Faz |
|------|-----|
| `spawn_agent(cli, model, task, mode)` | cria worktree+tmux, dispara o worker, retorna `agent_id` |
| `list_agents()` / `agent_status(id)` | estado/fase de cada worker |
| `read_output(id)` | saída acumulada do worker |
| `send_message(id, text)` | manda texto pra um worker (redirecionar / cochichar) |
| `pause_agent(id)` / `kill_agent(id)` | controle de ciclo de vida |
| `diff_agent(id)` | diff da branch do worker |
| `run_tests(id)` | roda testes/build no worktree, retorna resultado |
| `review(ids)` | monta contexto de revisão (diffs das N branches) |
| `open_pr(branch, body)` | abre PR via `gh` (ou fallback patch local) |

Playbooks (Q13=C) são instruções + atalhos que o mestre segue, não pipeline hardcoded.

### Triagem (2 camadas)

1. **Haiku** classifica a dificuldade (rápido/barato).
2. Se **difícil** ou **incerto** → **Opus** refina o plano (subtarefas, time, modo) antes de disparar.

### Os dois playbooks

**Fácil — dividir & conquistar**
1. Plano quebra em subtarefas complementares.
2. Worker barato + `codex` + `opencode` pegam **partes diferentes**.
3. **Merge-tudo**: junta todas as branches; Opus resolve conflitos de arquivo.
4. Opus revisa o conjunto; escala para Opus forte só se necessário.

**Difícil — redundância & juiz**
1. Todos os workers fazem **a mesma coisa**, isolados.
2. Opus lê as N versões e **funde o melhor de cada** (merge sintético).
3. **Fable** caça bugs em paralelo.
4. **Loop de conserto**: roda teste se existir, senão Opus julga. **Máx 3 rodadas.**

### Verificação (sinal do loop)

Híbrido: usa teste/build objetivo quando o projeto tem; senão julgamento do Opus com
teto rígido de rodadas (3). No modo difícil, Fable pode gerar um teste de repro rápido.

### Timeout / watchdog

Timeout **por papel** (configurável). Ao estourar: **mata e retenta 1x** (pode com modelo
mais rápido); se falhar de novo, descarta esse worker e segue com os que terminaram.
Se **todos** falharem → reporta erro.

### Health check (boot)

Antes de disparar um run, pré-voo dispara prompt trivial ("responde OK") a cada CLI do
roster (timeout ~15s). Bot morto → desabilita + avisa na TUI, não trava o resto.

### Segurança / permissões

**Sandbox + YOLO, zero prompt ao usuário** — totalmente autônomo. Auto-aprovação
(`--dangerously-skip-permissions` / `-y` / codex sandbox) confinada ao worktree via
sandbox + `--add-dir`. Isolamento de worktree + sandbox contém o estrago; a `main` nunca
é tocada direto.

### Saída: PR

**Nunca merga sozinho — abre um PR.** Configurável (Q16=C): default tenta `gh pr create`
(branch `regente/<slug>`, corpo escrito pelo mestre: o que fez, quais agentes, resumo da
revisão); sem `gh`/remote → cria branch + `.patch` local. Provider de PR também config
(GitHub/GitLab/nenhum). `gh` 2.96 autenticado como `Rafatakitani`.

### Remote control

- Interface é **TUI** (estilo Claude Code). Web fica como possível extra futuro, **não** prioridade.
- Remote sai do tmux: sessão do mestre roda persistente; do celular/outro device dá-se
  `ssh` + `tmux attach` (via Tailscale) e cai direto na TUI ao vivo — ver, mandar prompt,
  pausar, takeover de um worker. Mesmo mecanismo do painel ao vivo local.
- O usuário fala com o **mestre** (principal); opcionalmente cochicha direto num worker
  (`send_message`).

### Config (MUITO configurável)

- **Camadas** (Q15=C): global `~/.config/regente/config.yml` + por-projeto `.regente.yml`
  (projeto sobrescreve). Estilo settings global+local do Claude Code.
- **YAML** legível/comentável.
- **Editável ao vivo na TUI** (Q15=B): quase tudo ajustável por slash-command/telas de
  config dentro do app, que escrevem de volta nos YAMLs. TUI é a superfície principal.
- Configurável: roster (papel→CLI→modelo), qual é o mestre, timeouts por papel, playbooks,
  teto de rodadas, provider de PR, sandbox, atalhos.

## Stack

- **Ruby** (Q12). Fan-out por **threads** (trabalho é IO-bound — espera processos externos —
  então a GIL não atrapalha).
- Subprocess/PTY via tmux (shell out); `Open3` onde one-shot basta.
- git/gh/tmux via shell out. JSON via stdlib.
- TUI: a definir no plano (curses stdlib vs tty-toolkit) — decisão de implementação.
- MCP server em Ruby (transport stdio).

## Subsistemas (para o plano de implementação)

1. **Engine** — tmux + git worktree, spawn headless, captura, watchdog/timeout, health check.
2. **MCP server** — tools expostas ao mestre.
3. **Mestre + playbooks** — triagem 2 camadas, fácil/difícil, loop de revisão.
4. **TUI** — chat com o mestre, painéis de agente, takeover, telas de config.
5. **Config** — YAML em camadas + edição ao vivo.
6. **Remote** — persistência tmux + fluxo ssh/attach (majoritariamente grátis).
7. **PR/saída** — gh + fallback patch, corpo pelo mestre.

## Fora de escopo (por ora)

- Web UI (possível extra futuro).
- Merge automático (sempre PR + humano).
- Provedores além de GitHub/GitLab no PR.

## Decisões travadas (índice)

| # | Decisão |
|---|---------|
| Q1 | Envolver CLIs existentes headless (não reimplementar via API) |
| Q3 | Dashboard/cockpit + painel ao vivo sob demanda |
| Q4 | git worktree por agente |
| Q5 | Triagem em 2 camadas (haiku → opus) |
| Q6 | Fan-in = merge sintético + loop de conserto; verify híbrido; máx 3 |
| Q7 | Fácil = merge-tudo, Opus resolve conflito |
| Q9 | Timeout: mata e retenta 1x; health check no boot |
| Q10 | Sandbox + YOLO, zero prompt |
| Q11 | Alvo = repo git; entrada pela TUI/dir atual |
| Q12 | Ruby (threads, IO-bound) |
| Q13 | Mestre com tools + playbooks (híbrido); mestre trocável |
| Q14 | tmux por baixo (workers + TUI do mestre) |
| Q15 | Config YAML em camadas + edição ao vivo na TUI |
| Q16 | PR configurável: gh, fallback patch local |
| Q18 | Tool-calling via servidor MCP (Ruby) |
| — | Sem merge automático (abre PR); TUI (não web); sem MVP (constrói tudo) |
