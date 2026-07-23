# frozen_string_literal: true

require "test_helper"
require "stringio"
require "json"

class CLITest < Minitest::Test
  include TestHelpers

  def cli(**kw)
    Regente::CLI.new(stdin: StringIO.new, stdout: StringIO.new,
                     stderr: StringIO.new, **kw)
  end

  def test_version
    out = StringIO.new
    c = Regente::CLI.new(stdout: out, home: Dir.home, cwd: Dir.pwd)
    assert_equal 0, c.run(["version"])
    assert_includes out.string, Regente::VERSION
  end

  def test_help_when_no_args
    out = StringIO.new
    c = Regente::CLI.new(stdout: out)
    assert_equal 0, c.run([])
    assert_includes out.string, "orquestrador"
  end

  def test_doctor_uses_injected_prober
    with_temp_repo do |repo|
      out = StringIO.new
      prober = ->(cli) { cli == "claude" }
      c = Regente::CLI.new(stdout: out, home: repo, cwd: repo, probe_runner: prober)
      assert_equal 0, c.run(["doctor"])
      assert_includes out.string, "✓ claude"
      assert_includes out.string, "✗ codex"
    end
  end

  def test_launch_builds_master_command_in_tmux
    with_temp_repo do |repo|
      captured = nil
      launcher = ->(argv, _repo) { captured = argv }
      c = Regente::CLI.new(stdout: StringIO.new, home: repo, cwd: repo, launcher: launcher)
      assert_equal 0, c.run(["corrige", "o", "bug"])
      # wrapped in a persistent tmux session
      assert_equal "tmux", captured[0]
      assert_includes captured, "regente-master"
      master_cmd = captured.last
      assert_includes master_cmd, "claude"
      assert_includes master_cmd, "--mcp-config"
      # task is shell-escaped into the command
      assert_includes master_cmd, "corrige"
      assert_includes master_cmd, "bug"
    end
  end

  def test_config_get_default
    with_temp_repo do |repo|
      out = StringIO.new
      c = Regente::CLI.new(stdout: out, home: repo, cwd: repo)
      assert_equal 0, c.run(["config", "get", "master.cli"])
      assert_includes out.string, "claude"
    end
  end

  def test_config_set_persists_to_project
    with_temp_repo do |repo|
      c = Regente::CLI.new(stdout: StringIO.new, home: repo, cwd: repo)
      assert_equal 0, c.run(["config", "set", "timeouts.worker", "120"])
      # reload proves it persisted as an Integer
      reloaded = Regente::Config.load(project_dir: repo, home: repo)
      assert_equal 120, reloaded.timeouts["worker"]
    end
  end

  def test_mcp_serve_runs_server_on_stdio
    with_temp_repo do |repo|
      input = StringIO.new(JSON.generate({ jsonrpc: "2.0", id: 1, method: "initialize" }))
      output = StringIO.new
      c = Regente::CLI.new(stdin: input, stdout: output, home: repo, cwd: repo)
      assert_equal 0, c.run(["mcp-serve", "--repo", repo])
      reply = JSON.parse(output.string.each_line.first)
      assert_equal "regente", reply["result"]["serverInfo"]["name"]
    end
  end
end
