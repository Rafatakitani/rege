# frozen_string_literal: true

require "json"

module Regente
  module MCP
    # Minimal MCP server over stdio using newline-delimited JSON-RPC 2.0.
    # Any MCP-capable master (claude/codex/gemini/opencode) connects to this
    # and drives Regente through the tool catalog.
    class Server
      PROTOCOL_VERSION = "2024-11-05"

      def initialize(session:, input: $stdin, output: $stdout)
        @session = session
        @input = input
        @output = output
      end

      def run
        @input.each_line do |line|
          line = line.strip
          next if line.empty?

          msg = begin
            JSON.parse(line)
          rescue JSON::ParserError
            next
          end
          reply = handle(msg)
          write(reply) if reply
        end
      end

      # Handle one message; returns a response hash, or nil for notifications.
      def handle(msg)
        id = msg["id"]
        method = msg["method"]
        params = msg["params"] || {}

        # notifications carry no id and get no response
        return nil if id.nil? && method.to_s.start_with?("notifications/")

        case method
        when "initialize"
          ok(id, initialize_result)
        when "ping"
          ok(id, {})
        when "tools/list"
          ok(id, { tools: Tools.definitions })
        when "tools/call"
          ok(id, call_tool(params))
        else
          err(id, -32_601, "method not found: #{method}")
        end
      end

      private

      def initialize_result
        {
          protocolVersion: PROTOCOL_VERSION,
          capabilities: { tools: {} },
          serverInfo: { name: "regente", version: Regente::VERSION }
        }
      end

      def call_tool(params)
        name = params["name"]
        args = params["arguments"]
        result = Tools.call(name, args, @session)
        {
          content: [{ type: "text", text: JSON.generate(result) }],
          isError: result.is_a?(Hash) && !result[:error].nil?
        }
      rescue Regente::Error => e
        { content: [{ type: "text", text: e.message }], isError: true }
      end

      def ok(id, result)
        { jsonrpc: "2.0", id: id, result: result }
      end

      def err(id, code, message)
        { jsonrpc: "2.0", id: id, error: { code: code, message: message } }
      end

      def write(obj)
        @output.puts(JSON.generate(obj))
        @output.flush
      end
    end
  end
end
