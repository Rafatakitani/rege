# frozen_string_literal: true

require "test_helper"

class ScreenTest < Minitest::Test
  def agents
    [Regente::Dashboard::Row.new(name: "a1", state: :running, last: "editando"),
     Regente::Dashboard::Row.new(name: "a2", state: :done, last: "pronto")]
  end

  def chat
    [{ role: :user, text: "corrige bug" },
     { role: :assistant, text: "● analisando" },
     { role: :tool, text: "  ⚙ spawn_agent" }]
  end

  def frame(**over)
    Regente::Screen.compose(**{
      width: 80, height: 20, theme: "hacker", master: "claude · sonnet",
      repo: "/home/rafa/app", chat: chat, agents: agents, input: "oi", color: false
    }.merge(over))
  end

  def rows(f) = f.sub(/\A\e\[H/, "").split("\r\n")

  def test_frame_has_exact_height
    assert_equal 20, rows(frame).size
  end

  def test_contains_regions
    f = frame
    assert_includes f, "REGENTE"
    assert_includes f, "corrige bug"
    assert_includes f, "AGENTES"
    assert_includes f, "a1"
    assert_includes f, "running"
  end

  def test_input_line_last_with_prompt_and_cursor
    last = rows(frame).last
    assert_includes last, "root@regente:~#"
    assert_includes last, "oi"
    assert_includes last, "█"
  end

  def test_truncates_long_text_to_width
    long = "x" * 500
    f = frame(chat: [{ role: :assistant, text: long }])
    rows(f).each { |line| assert_operator line.length, :<=, 80 + 4 } # margin + clear code slack
  end

  def test_color_wraps_ansi_when_enabled
    f = frame(color: true)
    assert_includes f, "\e[38;2;0;255;65m" # hacker accent
  end

  def test_agents_empty_shows_placeholder
    assert_includes frame(agents: []), "nenhum agente ativo"
  end
end
