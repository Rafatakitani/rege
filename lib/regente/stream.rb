# frozen_string_literal: true

module Regente
  # Translates Claude's stream-json events into small, UI-friendly Regente
  # events so our own TUI can render the conversation (instead of handing off
  # to an external CLI's interface). Pure: given one parsed JSON hash, returns
  # an array of Event (a hash may carry several content blocks).
  module Stream
    Event = Struct.new(:type, :text, :name, :input, :cost, :session_id, keyword_init: true)

    module_function

    def parse(hash)
      case hash["type"]
      when "system"
        hash["subtype"] == "init" ? [Event.new(type: :ready, session_id: hash["session_id"])] : []
      when "assistant"
        blocks(hash.dig("message", "content"))
      when "user"
        tool_results(hash.dig("message", "content"))
      when "result"
        [Event.new(type: :done, cost: hash["total_cost_usd"], text: hash["result"],
                   session_id: hash["session_id"])]
      else
        []
      end
    end

    def blocks(content)
      return [] unless content.is_a?(Array)

      content.filter_map do |b|
        case b["type"]
        when "text"
          Event.new(type: :text, text: b["text"])
        when "tool_use"
          Event.new(type: :tool, name: b["name"], input: b["input"])
        end
      end
    end

    def tool_results(content)
      return [] unless content.is_a?(Array)

      content.filter_map do |b|
        next unless b["type"] == "tool_result"

        txt = b["content"]
        txt = txt.map { |c| c["text"] }.join if txt.is_a?(Array)
        Event.new(type: :tool_result, text: txt.to_s)
      end
    end
  end
end
