# frozen_string_literal: true

require "open3"
require "timeout"

module Regente
  # Orchestration engine: spawns/tracks agents, enforces per-role timeouts
  # (kill + retry once), and runs a boot health check over the roster.
  # Fan-out uses threads: the work is in external processes, so the GIL
  # does not bottleneck (Q12).
  class Engine
    attr_reader :repo, :config, :agents

    def initialize(repo:, config:, probe_runner: nil)
      @repo = repo
      @config = config
      @agents = []
      # probe_runner.(cli) => true/false ; injectable for tests
      @probe_runner = probe_runner || method(:default_probe)
    end

    # Probe each distinct CLI in the roster; returns { "claude" => true, ... }.
    def health_check
      clis = @config.roster.map { |r| r["cli"] }.uniq
      clis.each_with_object({}) do |cli, acc|
        acc[cli] = begin
          @probe_runner.call(cli)
        rescue StandardError
          false
        end
      end
    end

    # Create + start an agent, tracked by the engine.
    def spawn(name:, cli:, task:, model: nil, role: "worker", base: nil, command: nil)
      agent = Agent.new(repo: @repo, name: name, cli: cli, task: task, model: model,
                        role: role, config: @config, base: base, command: command)
      agent.start
      @agents << agent
      agent
    end

    # Run agents concurrently, each under its role timeout. On timeout, kill and
    # retry once (Q9). Returns the agents (with final state) that succeeded.
    def run_all(agents = @agents)
      threads = agents.map do |agent|
        Thread.new { supervise(agent) }
      end
      threads.each(&:join)
      agents.select { |a| a.state == :done }
    end

    def shutdown
      @agents.each(&:cleanup)
      @agents.clear
    end

    private

    def supervise(agent)
      timeout = timeout_for(agent.role)
      state = agent.wait(timeout: timeout)
      if state == :timeout && retries_allowed?
        agent.tmux.kill
        restart(agent)
        agent.wait(timeout: timeout)
      end
      agent.commit if agent.state == :done
      agent.state
    end

    def restart(agent)
      # fresh session, same worktree, retry the same command
      cmd = agent.instance_variable_get(:@command) ||
            Command.string(cli: agent.cli, task: agent.task, model: agent.model)
      new_tmux = Tmux.new(session: "regente-#{agent.name}-retry")
      agent.instance_variable_set(:@tmux, new_tmux)
      new_tmux.start(cmd, cwd: agent.worktree.path)
      agent.instance_variable_set(:@state, :running)
    end

    def timeout_for(role)
      @config.timeouts[role] || @config.timeouts["worker"]
    end

    def retries_allowed?
      (@config.playbooks["retry_on_timeout"] || 0).positive?
    end

    def default_probe(cli)
      timeout = @config.timeouts["healthcheck"] || 15
      cmd = Command.probe_string(cli)
      Timeout.timeout(timeout + 2) do
        _o, _e, status = Open3.capture3(*cmd.shellsplit)
        status.success?
      end
    rescue Timeout::Error, StandardError
      false
    end
  end
end
