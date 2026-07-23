# frozen_string_literal: true

require "test_helper"

class TmuxTest < Minitest::Test
  def setup
    skip "tmux nao instalado" unless system("tmux -V > /dev/null 2>&1")
    @sessions = []
  end

  def teardown
    @sessions.each { |s| s.kill if s&.alive? }
  end

  def new_session(name)
    s = Regente::Tmux.new(session: "regente-test-#{name}-#{Process.pid}")
    @sessions << s
    s
  end

  def test_start_runs_command_and_captures_output
    s = new_session("echo")
    s.start(%(echo hello-regente), cwd: Dir.pwd)
    assert s.wait(timeout: 5), "sessao deveria terminar"
    assert_includes s.output, "hello-regente"
    assert_equal 0, s.exit_code
  end

  def test_exit_code_propagates
    s = new_session("fail")
    s.start(%(exit 3), cwd: Dir.pwd)
    assert s.wait(timeout: 5)
    assert_equal 3, s.exit_code
  end

  def test_alive_while_running_then_dead
    s = new_session("sleep")
    s.start(%(sleep 0.5), cwd: Dir.pwd)
    assert s.alive?
    assert s.wait(timeout: 5)
    refute s.done? == false # done detected
  end

  def test_send_injects_keys_takeover
    s = new_session("interactive")
    s.start(%(read x; echo got-$x), cwd: Dir.pwd)
    s.send("banana")
    assert s.wait(timeout: 5)
    assert_includes s.output, "got-banana"
  end
end
