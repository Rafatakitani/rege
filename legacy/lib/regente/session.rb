# frozen_string_literal: true

require "open3"

module Regente
  # The stateful context the master operates on through MCP tools. Wraps the
  # Engine + PR and tracks agents by id. All methods return plain Ruby hashes
  # (JSON-friendly) so the MCP layer can serialize them directly.
  class Session
    attr_reader :repo, :config, :engine, :pr

    def initialize(repo:, config:, engine: nil, pr: nil)
      @repo = repo
      @config = config
      @engine = engine || Engine.new(repo: repo, config: config)
      @pr = pr || PR.new(repo: repo, config: config)
      @counter = 0
    end

    def spawn_agent(cli:, task:, model: nil, role: "worker", command: nil)
      @counter += 1
      name = "a#{@counter}"
      agent = @engine.spawn(name: name, cli: cli, task: task, model: model,
                            role: role, command: command)
      { agent_id: name, branch: agent.branch, state: agent.state }
    end

    def list_agents
      { agents: @engine.agents.map { |a| { agent_id: a.name, cli: a.cli, model: a.model, role: a.role, state: a.refresh } } }
    end

    def agent_status(agent_id:)
      with_agent(agent_id) do |a|
        state = a.refresh
        a.commit if state == :done # persist worker's work so diff/collect works
        { agent_id: agent_id, state: state }
      end
    end

    # Block until the agent finishes (or timeout), then commit its work.
    # Lets a one-shot master orchestrate without a polling loop.
    def wait_agent(agent_id:, timeout: 300)
      with_agent(agent_id) do |a|
        state = a.wait(timeout: timeout)
        a.commit if state == :done
        { agent_id: agent_id, state: state }
      end
    end

    def read_output(agent_id:)
      with_agent(agent_id) { |a| { agent_id: agent_id, output: a.output } }
    end

    def send_message(agent_id:, text:)
      with_agent(agent_id) do |a|
        a.send(text)
        { agent_id: agent_id, sent: true }
      end
    end

    def kill_agent(agent_id:)
      with_agent(agent_id) do |a|
        a.tmux.kill
        { agent_id: agent_id, killed: true }
      end
    end

    def diff_agent(agent_id:)
      with_agent(agent_id) do |a|
        a.commit if a.refresh == :done # ensure committed before diffing
        { agent_id: agent_id, diff: a.diff }
      end
    end

    def review(agent_ids:)
      diffs = agent_ids.map do |id|
        a = find(id)
        { agent_id: id, branch: a&.branch, diff: a&.diff }
      end
      { review: diffs }
    end

    # Run the configured verify command inside an agent's worktree (Q6.1 hybrid).
    def run_tests(agent_id:)
      cmd = @config.get("verify.command")
      return { skipped: true, reason: "sem verify.command configurado" } unless cmd

      with_agent(agent_id) do |a|
        out, _e, status = Open3.capture3(cmd, chdir: a.worktree.path)
        { agent_id: agent_id, passed: status.success?, output: out }
      end
    end

    # Ask a stronger model a one-shot question (escalation without spawning a
    # full worker). Used when the sonnet master hits a decision beyond it.
    def consult(question:, model: "opus", cli: "claude")
      cmd = Command.string(cli: cli, task: question, model: model, yolo: false)
      out, _e, status = Open3.capture3(*cmd.shellsplit, chdir: @repo)
      { model: model, answer: out.strip, ok: status.success? }
    end

    def open_pr(branch:, title:, body:)
      result = @pr.open(branch: branch, title: title, body: body)
      { mode: result.mode, ref: result.ref }
    end

    private

    def find(id) = @engine.agents.find { |a| a.name == id }

    def with_agent(id)
      a = find(id)
      return { error: "agente inexistente: #{id}" } unless a

      yield a
    end
  end
end
