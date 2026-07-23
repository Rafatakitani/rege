# frozen_string_literal: true

require "shellwords"

module Regente
  # Builds the headless, auto-approving invocation for each supported CLI.
  # Everything runs confined to the agent's worktree cwd; yolo flags remove
  # interactive prompts (Q10: sandbox + zero prompt).
  module Command
    module_function

    def string(cli:, task:, model: nil, yolo: true)
      argv(cli: cli, task: task, model: model, yolo: yolo).shelljoin
    end

    def argv(cli:, task:, model: nil, yolo: true)
      case cli
      when "claude"
        a = ["claude", "-p", task]
        a += ["--model", model] if model
        a << "--dangerously-skip-permissions" if yolo
        a
      when "codex"
        a = ["codex", "exec"]
        a += ["-m", model] if model
        a << "--dangerously-bypass-approvals-and-sandbox" if yolo
        a << task
        a
      when "gemini"
        a = ["gemini", "-p", task]
        a += ["-m", model] if model
        a << "--yolo" if yolo
        a
      when "opencode"
        a = ["opencode", "run"]
        a += ["--model", model] if model
        a << task
        a
      else
        raise Error, "CLI desconhecido no roster: #{cli}"
      end
    end

    # Trivial liveness probe for health checks.
    def probe_string(cli)
      string(cli: cli, task: "reply with OK", model: nil)
    end
  end

  # A single worker: a CLI+model doing a task, isolated in a worktree, running
  # inside a tmux session. Wraps lifecycle + status.
  class Agent
    STATES = %i[pending running done failed timeout].freeze

    attr_reader :name, :cli, :model, :task, :role, :worktree, :tmux, :state

    def initialize(repo:, name:, cli:, task:, model: nil, role: "worker",
                   config: nil, base: nil, command: nil)
      @repo = repo
      @name = name
      @cli = cli
      @model = model
      @task = task
      @role = role
      @yolo = config ? config.sandbox["yolo"] : true
      prefix = config ? config.pr["branch_prefix"] : "regente"
      @worktree = Worktree.new(repo: repo, name: name, branch_prefix: prefix, base: base)
      @tmux = Tmux.new(session: "regente-#{name}")
      @command = command # test/override hook; otherwise built from cli
      @state = :pending
    end

    def start
      @worktree.create
      cmd = @command || Command.string(cli: @cli, task: @task, model: @model, yolo: @yolo)
      @tmux.start(cmd, cwd: @worktree.path)
      @state = :running
      self
    end

    # Poll status; returns one of STATES.
    def refresh
      return @state unless @state == :running
      return @state unless @tmux.done?

      @state = @tmux.exit_code.to_i.zero? ? :done : :failed
    end

    def wait(timeout:)
      finished = @tmux.wait(timeout: timeout)
      unless finished
        @state = :timeout
        return :timeout
      end
      refresh
    end

    def output = @tmux.output
    def snapshot = @tmux.snapshot
    def exit_code = @tmux.exit_code
    def send(text) = @tmux.send(text)

    # Commit the agent's work so the branch carries a diff for review.
    def commit(message = "regente: #{@name}")
      @worktree.commit_all(message)
    rescue Error
      nil # nothing to commit
    end

    def diff = @worktree.diff
    def branch = @worktree.branch

    def cleanup
      @tmux.kill if @tmux.alive?
      @worktree.remove
    end
  end
end
