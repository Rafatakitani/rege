# frozen_string_literal: true

require "json"
require "shellwords"

module Regente
  # Launches the conversational master CLI wired to the Regente MCP server and
  # seeded with the playbook system prompt. The master runs interactively
  # (inside tmux for persistence + remote attach); it connects back to
  # `regente mcp-serve --repo <repo>` over stdio for its tools.
  module Master
    module_function

    # MCP server registration the master connects to.
    def mcp_config(repo:, exe: "regente")
      {
        mcpServers: {
          regente: {
            command: exe,
            args: ["mcp-serve", "--repo", repo]
          }
        }
      }
    end

    # Build the argv to launch a given master CLI interactively.
    def launch_argv(cli:, repo:, prompt:, model: nil, task: nil, exe: "regente")
      cfg_json = JSON.generate(mcp_config(repo: repo, exe: exe))
      case cli
      when "claude"
        a = ["claude", "--mcp-config", cfg_json, "--append-system-prompt", prompt]
        a += ["--model", model] if model
        a << task if task # seed the interactive session with the first task
        a
      when "gemini"
        # gemini reads MCP servers from its settings; pass prompt-interactive.
        seed = task ? "#{prompt}\n\nTarefa: #{task}" : prompt
        a = ["gemini", "-i", seed]
        a += ["-m", model] if model
        a
      when "codex"
        a = ["codex"]
        a += ["-m", model] if model
        a << task if task
        a
      when "opencode"
        task ? ["opencode", task] : ["opencode"]
      else
        raise Error, "master CLI nao suportado: #{cli}"
      end
    end

    def launch_string(cli:, repo:, prompt:, model: nil, task: nil, exe: "regente")
      launch_argv(cli: cli, repo: repo, prompt: prompt, model: model,
                  task: task, exe: exe).shelljoin
    end
  end
end
