# frozen_string_literal: true

require "open3"

module Regente
  # Live worker status for the bottom-half dashboard. Workers run in a separate
  # process (the master's MCP server), so the shared registry both sides see is
  # tmux: sessions named `regente-<name>`. We read each worker's log file for
  # its state. Cross-process, no shared memory.
  class Dashboard
    Row = Struct.new(:name, :state, :last, keyword_init: true)

    def initialize(lister: nil, logdir: nil, master_session: "regente-master")
      @lister = lister || method(:default_lister)
      @logdir = logdir || File.join(Dir.tmpdir, "regente-logs")
      @master_session = master_session
    end

    # Worker rows, master session excluded.
    def workers
      @lister.call
             .select { |s| s.start_with?("regente-") }
             .reject { |s| s == @master_session }
             .map { |s| row_for(s) }
    end

    def row_for(session)
      name = session.sub(/\Aregente-/, "")
      log = read_log(session)
      Row.new(name: name, state: state_from(session, log), last: last_line(log))
    end

    private

    def state_from(session, log)
      if (m = log.match(/__RG_EXIT_(\d+)__/))
        m[1].to_i.zero? ? :done : :failed
      elsif alive?(session)
        :running
      else
        :unknown
      end
    end

    def last_line(log)
      log.gsub(/__RG_EXIT_\d+__/, "").each_line.map(&:strip).reject(&:empty?).last.to_s
    end

    def read_log(session)
      File.read(File.join(@logdir, "#{session}.log"))
    rescue Errno::ENOENT
      ""
    end

    def alive?(session)
      system("tmux", "has-session", "-t", session, out: File::NULL, err: File::NULL)
    end

    def default_lister
      out, _e, status = Open3.capture3("tmux", "list-sessions", "-F", "#S")
      return [] unless status.success?

      out.each_line.map(&:strip).reject(&:empty?)
    end
  end
end
