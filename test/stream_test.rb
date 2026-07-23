# frozen_string_literal: true

require "test_helper"

class StreamTest < Minitest::Test
  def test_init_becomes_ready_with_session
    events = Regente::Stream.parse({ "type" => "system", "subtype" => "init",
                                     "session_id" => "abc" })
    assert_equal :ready, events[0].type
    assert_equal "abc", events[0].session_id
  end

  def test_assistant_text_block
    events = Regente::Stream.parse({
      "type" => "assistant",
      "message" => { "content" => [{ "type" => "text", "text" => "oi" }] }
    })
    assert_equal :text, events[0].type
    assert_equal "oi", events[0].text
  end

  def test_assistant_tool_use_block
    events = Regente::Stream.parse({
      "type" => "assistant",
      "message" => { "content" => [
        { "type" => "text", "text" => "vou spawnar" },
        { "type" => "tool_use", "name" => "spawn_agent", "input" => { "cli" => "codex" } }
      ] }
    })
    assert_equal %i[text tool], events.map(&:type)
    assert_equal "spawn_agent", events[1].name
    assert_equal "codex", events[1].input["cli"]
  end

  def test_tool_result_from_user
    events = Regente::Stream.parse({
      "type" => "user",
      "message" => { "content" => [
        { "type" => "tool_result", "content" => [{ "type" => "text", "text" => "done" }] }
      ] }
    })
    assert_equal :tool_result, events[0].type
    assert_equal "done", events[0].text
  end

  def test_result_becomes_done_with_cost
    events = Regente::Stream.parse({ "type" => "result", "total_cost_usd" => 0.02,
                                     "result" => "pronto", "session_id" => "s" })
    assert_equal :done, events[0].type
    assert_in_delta 0.02, events[0].cost
  end

  def test_unknown_type_ignored
    assert_empty Regente::Stream.parse({ "type" => "rate_limit_event" })
  end
end

class MasterDriverTest < Minitest::Test
  CANNED = [
    { type: "system", subtype: "init", session_id: "sess-1" },
    { type: "assistant", message: { content: [{ type: "text", text: "analisando" }] } },
    { type: "assistant", message: { content: [{ type: "tool_use", name: "spawn_agent",
                                                input: { cli: "codex", task: "x" } }] } },
    { type: "result", total_cost_usd: 0.01, result: "feito", session_id: "sess-1" }
  ].map { |h| JSON.generate(h) }.join("\n")

  def test_only_claude_supported
    assert_raises(Regente::Error) do
      Regente::MasterDriver.new(cli: "gemini", repo: "/x", prompt: "p")
    end
  end

  def test_send_turn_yields_events_and_captures_session
    argvs = []
    runner = lambda do |argv, &blk|
      argvs << argv
      CANNED.each_line { |l| blk.call(l) }
    end
    d = Regente::MasterDriver.new(cli: "claude", repo: "/x", prompt: "PLAY", runner: runner)
    got = []
    d.send_turn("corrige") { |ev| got << ev }
    assert_equal %i[ready text tool done], got.map(&:type)
    assert_equal "sess-1", d.session_id
    # first turn seeds system prompt, no resume
    assert_includes argvs[0], "--append-system-prompt"
    refute_includes argvs[0], "--resume"
  end

  def test_second_turn_uses_resume
    argvs = []
    runner = lambda do |argv, &blk|
      argvs << argv
      CANNED.each_line { |l| blk.call(l) }
    end
    d = Regente::MasterDriver.new(cli: "claude", repo: "/x", prompt: "PLAY", runner: runner)
    d.send_turn("um") { |_| }
    d.send_turn("dois") { |_| }
    assert_includes argvs[1], "--resume"
    assert_includes argvs[1], "sess-1"
    refute_includes argvs[1], "--append-system-prompt"
  end
end
