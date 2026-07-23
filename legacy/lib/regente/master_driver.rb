# frozen_string_literal: true

require "json"
require "open3"

module Regente
  # Drives the master model headless in streaming mode and yields Regente
  # events, so OUR TUI renders the conversation. Multi-turn is done by
  # re-spawning with --resume <session_id> (robust, no long-lived stdin
  # protocol). Claude is the streaming engine; other CLIs use `regente attach`.
  class MasterDriver
    attr_reader :session_id

    def initialize(cli:, repo:, prompt:, model: nil, exe: "regente", runner: nil)
      raise Error, "chat streaming so suporta claude (use `regente attach`)" unless cli == "claude"

      @repo = repo
      @prompt = prompt
      @model = model
      @exe = exe
      @session_id = nil
      @runner = runner || method(:default_runner)
    end

    # Send one user turn; yields Stream::Event as they arrive.
    def send_turn(text)
      first = @session_id.nil?
      @runner.call(build_argv(text, first)) do |line|
        line = line.strip
        next if line.empty?

        hash = begin
          JSON.parse(line)
        rescue JSON::ParserError
          next
        end
        Stream.parse(hash).each do |ev|
          @session_id = ev.session_id if ev.session_id
          yield ev
        end
      end
      self
    end

    def build_argv(text, first)
      mcp = JSON.generate(Master.mcp_config(repo: @repo, exe: @exe))
      argv = ["claude", "-p", text,
              "--output-format", "stream-json", "--verbose",
              "--dangerously-skip-permissions",
              "--mcp-config", mcp]
      if first
        argv += ["--append-system-prompt", @prompt]
      else
        argv += ["--resume", @session_id]
      end
      argv += ["--model", @model] if @model
      argv
    end

    private

    def default_runner(argv, &blk)
      Open3.popen2e(*argv, chdir: @repo) do |stdin, out, wait|
        stdin.close
        out.each_line(&blk)
        wait.value
      end
    end
  end
end
