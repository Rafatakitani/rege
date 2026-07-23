# frozen_string_literal: true

require "test_helper"
require "stringio"

class TUITest < Minitest::Test
  include TestHelpers

  def tui(repo)
    cfg = Regente::Config.load(project_dir: repo, home: repo)
    Regente::TUI.new(config: cfg, repo: repo, out: StringIO.new, color: false)
  end

  def test_banner_has_brand
    with_temp_repo { |r| assert_includes tui(r).banner, "██" }
  end

  def test_header_box_shows_master_and_repo
    with_temp_repo do |repo|
      box = tui(repo).header_box
      assert_includes box, "claude"
      assert_includes box, "opus"
      assert_includes box, File.basename(repo)
    end
  end

  def test_health_lines_mark_ok_and_fail
    with_temp_repo do |repo|
      lines = tui(repo).health_lines({ "claude" => true, "codex" => false })
      assert_includes lines, "claude"
      assert_includes lines, "codex"
      assert_includes lines, "sem resposta"
    end
  end

  def test_footer_lists_commands
    with_temp_repo do |repo|
      f = tui(repo).footer
      assert_includes f, "/doctor"
      assert_includes f, "/quit"
    end
  end

  def test_format_event_text_and_tool_and_done
    with_temp_repo do |repo|
      t = tui(repo)
      txt = t.format_event(Regente::Stream::Event.new(type: :text, text: "oi"))
      assert_includes txt, "oi"
      tool = t.format_event(Regente::Stream::Event.new(type: :tool, name: "spawn_agent",
                                                       input: { "cli" => "codex" }))
      assert_includes tool, "spawn_agent"
      assert_includes tool, "cli=codex"
      done = t.format_event(Regente::Stream::Event.new(type: :done, cost: 0.0123))
      assert_includes done, "0.0123"
    end
  end

  def test_chat_streams_driver_events_into_output
    with_temp_repo do |repo|
      cfg = Regente::Config.load(project_dir: repo, home: repo)
      out = StringIO.new
      fake_driver = Object.new
      events = [Regente::Stream::Event.new(type: :text, text: "analisando"),
                Regente::Stream::Event.new(type: :tool, name: "spawn_agent", input: {}),
                Regente::Stream::Event.new(type: :done, cost: 0.02)]
      fake_driver.define_singleton_method(:send_turn) do |_text, &blk|
        events.each(&blk)
      end
      t = Regente::TUI.new(config: cfg, repo: repo, out: out, color: false, driver: fake_driver)
      t.send(:chat, "corrige o bug")
      assert_includes out.string, "corrige o bug" # user echo
      assert_includes out.string, "analisando"    # assistant text
      assert_includes out.string, "spawn_agent"   # tool activity
    end
  end

  def test_bare_cli_non_tty_shows_usage_not_tui
    with_temp_repo do |repo|
      out = StringIO.new # not a tty -> must not enter Reline loop
      c = Regente::CLI.new(stdout: out, home: repo, cwd: repo)
      assert_equal 0, c.run([])
      assert_includes out.string, "orquestrador"
    end
  end
end
