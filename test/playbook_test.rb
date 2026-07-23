# frozen_string_literal: true

require "test_helper"
require "json"

class PlaybookTest < Minitest::Test
  include TestHelpers

  def cfg(repo)
    Regente::Config.load(project_dir: repo, home: repo)
  end

  def test_prompt_encodes_policy_and_rounds
    with_temp_repo do |repo|
      c = cfg(repo)
      c.set("playbooks.review_rounds", 5)
      prompt = Regente::Playbook.prompt(c)
      assert_includes prompt, "TRIAGEM"
      assert_includes prompt, "MAXIMO 5 rodadas"
      assert_includes prompt, "NUNCA faca merge"
      assert_includes prompt, "open_pr"
    end
  end

  def test_prompt_lists_roster
    with_temp_repo do |repo|
      prompt = Regente::Playbook.prompt(cfg(repo))
      assert_includes prompt, "triage: claude -> haiku"
      assert_includes prompt, "bughunter: claude -> fable"
    end
  end
end

class MasterTest < Minitest::Test
  def test_mcp_config_points_to_serve
    cfg = Regente::Master.mcp_config(repo: "/tmp/proj")
    server = cfg[:mcpServers][:regente]
    assert_equal "regente", server[:command]
    assert_equal ["mcp-serve", "--repo", "/tmp/proj"], server[:args]
  end

  def test_launch_argv_claude_wires_mcp_and_prompt
    argv = Regente::Master.launch_argv(cli: "claude", repo: "/tmp/proj",
                                       prompt: "PLAYBOOK", model: "opus")
    assert_includes argv, "--mcp-config"
    assert_includes argv, "--append-system-prompt"
    assert_includes argv, "PLAYBOOK"
    assert_includes argv, "opus"
    # the mcp-config json must reference our serve command
    json = argv[argv.index("--mcp-config") + 1]
    assert_includes json, "mcp-serve"
  end

  def test_unknown_master_raises
    assert_raises(Regente::Error) do
      Regente::Master.launch_argv(cli: "nope", repo: "/x", prompt: "p")
    end
  end
end
