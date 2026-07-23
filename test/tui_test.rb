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

  def test_bare_cli_non_tty_shows_usage_not_tui
    with_temp_repo do |repo|
      out = StringIO.new # not a tty -> must not enter Reline loop
      c = Regente::CLI.new(stdout: out, home: repo, cwd: repo)
      assert_equal 0, c.run([])
      assert_includes out.string, "orquestrador"
    end
  end
end
