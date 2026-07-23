# frozen_string_literal: true

require "test_helper"

class CommandTest < Minitest::Test
  def test_claude_headless_with_model_and_yolo
    argv = Regente::Command.argv(cli: "claude", task: "fix bug", model: "opus", yolo: true)
    assert_equal ["claude", "-p", "fix bug", "--model", "opus",
                  "--dangerously-skip-permissions"], argv
  end

  def test_claude_without_model_or_yolo
    argv = Regente::Command.argv(cli: "claude", task: "t", model: nil, yolo: false)
    assert_equal ["claude", "-p", "t"], argv
  end

  def test_codex_exec
    argv = Regente::Command.argv(cli: "codex", task: "t", model: "gpt", yolo: true)
    assert_equal ["codex", "exec", "-m", "gpt",
                  "--dangerously-bypass-approvals-and-sandbox", "t"], argv
  end

  def test_gemini_yolo
    argv = Regente::Command.argv(cli: "gemini", task: "t", model: nil, yolo: true)
    assert_equal ["gemini", "-p", "t", "--yolo"], argv
  end

  def test_opencode_run
    argv = Regente::Command.argv(cli: "opencode", task: "t", model: "x", yolo: true)
    assert_equal ["opencode", "run", "--model", "x", "t"], argv
  end

  def test_unknown_cli_raises
    assert_raises(Regente::Error) { Regente::Command.argv(cli: "nope", task: "t") }
  end
end

class AgentTest < Minitest::Test
  include TestHelpers

  def setup
    skip "tmux nao instalado" unless system("tmux -V > /dev/null 2>&1")
  end

  def cfg(repo)
    Regente::Config.load(project_dir: repo, home: repo)
  end

  def test_agent_runs_fake_command_to_done
    with_temp_repo do |repo|
      a = Regente::Agent.new(repo: repo, name: "a1", cli: "claude", task: "x",
                             config: cfg(repo), command: "echo hi-alpha")
      a.start
      assert_equal :done, a.wait(timeout: 5)
      assert_includes a.output, "hi-alpha"
    ensure
      a&.cleanup
    end
  end

  def test_agent_commits_work_and_diffs
    with_temp_repo do |repo|
      a = Regente::Agent.new(repo: repo, name: "a2", cli: "claude", task: "x",
                             config: cfg(repo), command: "echo content > out.txt")
      a.start
      assert_equal :done, a.wait(timeout: 5)
      a.commit
      diff = a.diff
      assert_includes diff, "out.txt"
      # main repo untouched
      refute File.exist?(File.join(repo, "out.txt"))
    ensure
      a&.cleanup
    end
  end

  def test_failed_command_marks_failed
    with_temp_repo do |repo|
      a = Regente::Agent.new(repo: repo, name: "a3", cli: "claude", task: "x",
                             config: cfg(repo), command: "exit 1")
      a.start
      assert_equal :failed, a.wait(timeout: 5)
    ensure
      a&.cleanup
    end
  end
end
