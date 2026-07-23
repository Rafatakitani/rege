# frozen_string_literal: true

module Regente
  module MCP
    # Tool catalog exposed to the master over MCP, plus the dispatch that maps
    # a tool call to a Session method. This is how the master "commands all"
    # (Q13/Q18): the master picks a tool, the Session executes it.
    module Tools
      # name => [description, inputSchema, session_method]
      CATALOG = {
        "spawn_agent" => [
          "Dispara um worker (CLI de IA) isolado num git worktree pra uma tarefa.",
          { type: "object",
            properties: {
              cli: { type: "string", description: "claude|codex|gemini|opencode" },
              task: { type: "string" },
              model: { type: "string" },
              role: { type: "string" }
            },
            required: %w[cli task] },
          :spawn_agent
        ],
        "list_agents" => [
          "Lista os agentes e seus estados.",
          { type: "object", properties: {} },
          :list_agents
        ],
        "agent_status" => [
          "Estado atual de um agente.",
          { type: "object", properties: { agent_id: { type: "string" } }, required: %w[agent_id] },
          :agent_status
        ],
        "read_output" => [
          "Saida acumulada de um agente.",
          { type: "object", properties: { agent_id: { type: "string" } }, required: %w[agent_id] },
          :read_output
        ],
        "send_message" => [
          "Injeta texto na sessao de um agente (redirecionar / cochichar / takeover).",
          { type: "object",
            properties: { agent_id: { type: "string" }, text: { type: "string" } },
            required: %w[agent_id text] },
          :send_message
        ],
        "kill_agent" => [
          "Mata a sessao de um agente.",
          { type: "object", properties: { agent_id: { type: "string" } }, required: %w[agent_id] },
          :kill_agent
        ],
        "diff_agent" => [
          "Diff da branch de um agente.",
          { type: "object", properties: { agent_id: { type: "string" } }, required: %w[agent_id] },
          :diff_agent
        ],
        "review" => [
          "Monta o contexto de revisao: diffs das branches dos agentes dados.",
          { type: "object",
            properties: { agent_ids: { type: "array", items: { type: "string" } } },
            required: %w[agent_ids] },
          :review
        ],
        "run_tests" => [
          "Roda o comando de verify no worktree do agente (se configurado).",
          { type: "object", properties: { agent_id: { type: "string" } }, required: %w[agent_id] },
          :run_tests
        ],
        "consult" => [
          "Pergunta pontual a um modelo mais forte (ex: opus) sem spawnar worker. Escalacao de raciocinio.",
          { type: "object",
            properties: { question: { type: "string" }, model: { type: "string" } },
            required: %w[question] },
          :consult
        ],
        "open_pr" => [
          "Abre um PR a partir de uma branch (nunca faz merge). Fallback: patch local.",
          { type: "object",
            properties: { branch: { type: "string" }, title: { type: "string" }, body: { type: "string" } },
            required: %w[branch title body] },
          :open_pr
        ]
      }.freeze

      module_function

      # MCP tools/list payload.
      def definitions
        CATALOG.map do |name, (desc, schema, _method)|
          { name: name, description: desc, inputSchema: schema }
        end
      end

      # Dispatch a tool call to the session. Returns the session's result hash.
      # Raises Regente::Error on unknown tool.
      def call(name, arguments, session)
        entry = CATALOG[name]
        raise Error, "tool desconhecida: #{name}" unless entry

        method_name = entry[2]
        kwargs = symbolize(arguments || {})
        session.public_send(method_name, **kwargs)
      end

      def symbolize(hash)
        hash.each_with_object({}) { |(k, v), h| h[k.to_sym] = v }
      end
    end
  end
end
