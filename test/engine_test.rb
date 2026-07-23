# frozen_string_literal: true

require "test_helper"

class EngineTest < Minitest::Test
  include TestHelpers

  def setup
    skip "tmux nao instalado" unless system("tmux -V > /dev/null 2>&1")
  end

  def cfg(repo)
    Regente::Config.load(project_dir: repo, home: repo)
  end

  def test_health_check_uses_injected_prober
    with_temp_repo do |repo|
      prober = ->(cli) { cli == "claude" }
      eng = Regente::Engine.new(repo: repo, config: cfg(repo), probe_runner: prober)
      result = eng.health_check
      assert result["claude"]
      refute result["codex"]
      refute result["opencode"]
    end
  end

  def test_spawn_and_run_all_concurrent
    with_temp_repo do |repo|
      eng = Regente::Engine.new(repo: repo, config: cfg(repo))
      eng.spawn(name: "w1", cli: "claude", task: "x", command: "echo one > a.txt")
      eng.spawn(name: "w2", cli: "claude", task: "x", command: "echo two > b.txt")
      done = eng.run_all
      assert_equal 2, done.size
      assert(done.all? { |a| a.state == :done })
      # each committed its own file on its own branch
      assert_includes done.map(&:branch).sort, "regente/w1"
    ensure
      eng&.shutdown
    end
  end

  def test_timeout_marks_agent_timed_out
    with_temp_repo do |repo|
      c = cfg(repo)
      c.set("timeouts.worker", 1)
      c.set("playbooks.retry_on_timeout", 0)
      eng = Regente::Engine.new(repo: repo, config: c)
      eng.spawn(name: "slow", cli: "claude", task: "x", command: "sleep 5")
      done = eng.run_all
      assert_empty done
      assert_equal :timeout, eng.agents.first.state
    ensure
      eng&.shutdown
    end
  end
end
