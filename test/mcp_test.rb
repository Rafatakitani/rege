# frozen_string_literal: true

require "test_helper"
require "stringio"
require "json"

class MCPToolsTest < Minitest::Test
  def test_definitions_cover_catalog
    defs = Regente::MCP::Tools.definitions
    names = defs.map { |d| d[:name] }
    assert_includes names, "spawn_agent"
    assert_includes names, "open_pr"
    assert_equal Regente::MCP::Tools::CATALOG.size, defs.size
  end

  def test_call_dispatches_to_session
    fake = Object.new
    def fake.list_agents = { agents: [] }
    result = Regente::MCP::Tools.call("list_agents", {}, fake)
    assert_equal({ agents: [] }, result)
  end

  def test_call_symbolizes_arguments
    fake = Object.new
    def fake.agent_status(agent_id:) = { got: agent_id }
    result = Regente::MCP::Tools.call("agent_status", { "agent_id" => "a1" }, fake)
    assert_equal({ got: "a1" }, result)
  end

  def test_unknown_tool_raises
    assert_raises(Regente::Error) { Regente::MCP::Tools.call("nope", {}, nil) }
  end
end

class MCPServerTest < Minitest::Test
  include TestHelpers

  def roundtrip(messages, session:)
    input = StringIO.new(messages.map { |m| JSON.generate(m) }.join("\n"))
    output = StringIO.new
    Regente::MCP::Server.new(session: session, input: input, output: output).run
    output.string.each_line.map { |l| JSON.parse(l) }
  end

  def session(repo)
    cfg = Regente::Config.load(project_dir: repo, home: repo)
    Regente::Session.new(repo: repo, config: cfg)
  end

  def test_initialize_returns_server_info
    with_temp_repo do |repo|
      replies = roundtrip([{ jsonrpc: "2.0", id: 1, method: "initialize" }], session: session(repo))
      assert_equal "regente", replies[0]["result"]["serverInfo"]["name"]
      assert replies[0]["result"]["capabilities"]["tools"]
    end
  end

  def test_tools_list
    with_temp_repo do |repo|
      replies = roundtrip([{ jsonrpc: "2.0", id: 2, method: "tools/list" }], session: session(repo))
      names = replies[0]["result"]["tools"].map { |t| t["name"] }
      assert_includes names, "spawn_agent"
    end
  end

  def test_tools_call_list_agents_empty
    with_temp_repo do |repo|
      msg = { jsonrpc: "2.0", id: 3, method: "tools/call",
              params: { name: "list_agents", arguments: {} } }
      replies = roundtrip([msg], session: session(repo))
      text = replies[0]["result"]["content"][0]["text"]
      assert_equal({ "agents" => [] }, JSON.parse(text))
      refute replies[0]["result"]["isError"]
    end
  end

  def test_notification_produces_no_reply
    with_temp_repo do |repo|
      replies = roundtrip([{ jsonrpc: "2.0", method: "notifications/initialized" }], session: session(repo))
      assert_empty replies
    end
  end

  def test_unknown_method_errors
    with_temp_repo do |repo|
      replies = roundtrip([{ jsonrpc: "2.0", id: 9, method: "bogus/thing" }], session: session(repo))
      assert_equal(-32_601, replies[0]["error"]["code"])
    end
  end
end
