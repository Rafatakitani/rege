# frozen_string_literal: true

require "test_helper"

class ThemeTest < Minitest::Test
  def test_has_named_themes_not_just_dark_light
    assert_includes Regente::Theme.names, "hacker"
    assert_includes Regente::Theme.names, "luxury"
    assert_includes Regente::Theme.names, "cyberpunk"
    refute_includes Regente::Theme.names, "dark"
  end

  def test_color_returns_rgb_triple
    rgb = Regente::Theme.color("hacker", :accent)
    assert_equal [0, 255, 65], rgb
  end

  def test_unknown_theme_falls_back_to_default
    assert_equal Regente::Theme.palette("nope"), Regente::Theme.palette(Regente::Theme::DEFAULT)
  end

  def test_prompt_per_theme
    assert_equal "root@regente:~# ", Regente::Theme.prompt("hacker")
    assert_equal "λ ", Regente::Theme.prompt("dracula")
  end
end

class TUIThemeTest < Minitest::Test
  include TestHelpers

  def test_default_theme_is_hacker
    with_temp_repo do |repo|
      cfg = Regente::Config.load(project_dir: repo, home: repo)
      assert_equal "hacker", cfg.get("ui.theme")
    end
  end

  def test_tui_uses_configured_theme_color
    with_temp_repo do |repo|
      cfg = Regente::Config.load(project_dir: repo, home: repo)
      cfg.set("ui.theme", "cyberpunk")
      t = Regente::TUI.new(config: cfg, repo: repo, out: StringIO.new, color: true)
      # cyberpunk accent is magenta 255;42;109 -> must appear in a painted banner
      assert_includes t.banner, "38;2;255;42;109"
    end
  end

  def test_header_shows_theme_name
    with_temp_repo do |repo|
      cfg = Regente::Config.load(project_dir: repo, home: repo)
      t = Regente::TUI.new(config: cfg, repo: repo, out: StringIO.new, color: false)
      assert_includes t.header_box, "hacker"
    end
  end
end
