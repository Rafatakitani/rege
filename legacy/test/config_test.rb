# frozen_string_literal: true

require "test_helper"

class ConfigTest < Minitest::Test
  include TestHelpers

  def with_layers
    Dir.mktmpdir("regente-cfg") do |root|
      home = File.join(root, "home")
      project = File.join(root, "project")
      FileUtils.mkdir_p(File.join(home, ".config", "regente"))
      FileUtils.mkdir_p(project)
      yield home, project
    end
  end

  def test_defaults_when_no_files
    with_layers do |home, project|
      cfg = Regente::Config.load(project_dir: project, home: home)
      assert_equal "claude", cfg.master["cli"]
      assert cfg.roster.is_a?(Array)
      refute_empty cfg.roster
      assert cfg.timeouts["worker"] > 0
    end
  end

  def test_global_overrides_defaults
    with_layers do |home, project|
      File.write(File.join(home, ".config", "regente", "config.yml"),
                 "master:\n  cli: gemini\n  model: pro\n")
      cfg = Regente::Config.load(project_dir: project, home: home)
      assert_equal "gemini", cfg.master["cli"]
      assert_equal "pro", cfg.master["model"]
    end
  end

  def test_project_overrides_global
    with_layers do |home, project|
      File.write(File.join(home, ".config", "regente", "config.yml"),
                 "master:\n  cli: gemini\n")
      File.write(File.join(project, ".regente.yml"),
                 "master:\n  cli: codex\n")
      cfg = Regente::Config.load(project_dir: project, home: home)
      assert_equal "codex", cfg.master["cli"]
    end
  end

  def test_deep_merge_preserves_untouched_keys
    with_layers do |home, project|
      File.write(File.join(project, ".regente.yml"),
                 "timeouts:\n  worker: 999\n")
      cfg = Regente::Config.load(project_dir: project, home: home)
      assert_equal 999, cfg.timeouts["worker"]
      # master untouched -> still default
      assert_equal "claude", cfg.master["cli"]
    end
  end

  def test_dot_path_get
    with_layers do |home, project|
      cfg = Regente::Config.load(project_dir: project, home: home)
      assert_equal "claude", cfg.get("master.cli")
      assert_nil cfg.get("does.not.exist")
    end
  end

  def test_set_and_save_project_roundtrip
    with_layers do |home, project|
      cfg = Regente::Config.load(project_dir: project, home: home)
      cfg.set("master.cli", "opencode")
      cfg.set("timeouts.worker", 120)
      cfg.save_project
      reloaded = Regente::Config.load(project_dir: project, home: home)
      assert_equal "opencode", reloaded.master["cli"]
      assert_equal 120, reloaded.timeouts["worker"]
    end
  end
end
