# frozen_string_literal: true

require "open3"
require "tmpdir"
require "securerandom"

module Regente
  # A worker runs inside a detached tmux session. tmux gives us, for free:
  # a real PTY, persistence (survives app/ssh crash), live attach, and
  # keystroke injection for takeover. Output is teed to a log file via
  # pipe-pane so we can read it even after the pane exits.
  class Tmux
    EXIT_RE = /__RG_EXIT_(\d+)__/

    attr_reader :session, :logfile

    def initialize(session:, logdir: nil)
      @session = session
      logdir ||= File.join(Dir.tmpdir, "regente-logs")
      FileUtils.mkdir_p(logdir)
      @logfile = File.join(logdir, "#{session}.log")
      File.write(@logfile, "")
    end

    # Start `command` inside a fresh detached session in `cwd`.
    def start(command, cwd: Dir.pwd, width: 200, height: 50)
      run_tmux("new-session", "-d", "-s", @session, "-x", width.to_s, "-y", height.to_s, "-c", cwd)
      run_tmux("pipe-pane", "-o", "-t", @session, "cat >> #{shell_quote(@logfile)}")
      # subshell so a command that calls `exit` doesn't kill our shell before
      # the sentinel is written.
      wrapped = %(( #{command} ); printf '\\n__RG_EXIT_%s__\\n' "$?")
      run_tmux("send-keys", "-t", @session, wrapped, "Enter")
      self
    end

    # Inject keystrokes (used for redirect / takeover). Sends Enter after.
    def send(text, enter: true)
      run_tmux("send-keys", "-t", @session, text)
      run_tmux("send-keys", "-t", @session, "Enter") if enter
      self
    end

    # Whether the tmux session still exists.
    def alive?
      system("tmux", "has-session", "-t", @session,
             out: File::NULL, err: File::NULL)
    end

    # The command finished if the exit sentinel has been written.
    def done?
      log_contents.match?(EXIT_RE)
    end

    def exit_code
      m = log_contents.match(EXIT_RE)
      m && m[1].to_i
    end

    # Full captured output with the sentinel line stripped.
    def output
      log_contents.gsub(/__RG_EXIT_\d+__\s*/, "")
    end

    # Current visible pane snapshot (for live dashboards).
    def snapshot
      out, = Open3.capture2("tmux", "capture-pane", "-p", "-t", @session)
      out
    rescue StandardError
      ""
    end

    # Block until the command finishes (sentinel) or timeout (seconds).
    # Returns true if it finished, false on timeout.
    def wait(timeout: 300, poll: 0.05)
      deadline = monotonic + timeout
      until done?
        return false if monotonic > deadline

        sleep(poll)
      end
      true
    end

    def kill
      run_tmux("kill-session", "-t", @session)
    rescue StandardError
      nil
    end

    private

    def log_contents
      File.read(@logfile)
    rescue Errno::ENOENT
      ""
    end

    def monotonic = Process.clock_gettime(Process::CLOCK_MONOTONIC)

    def run_tmux(*args)
      _o, err, status = Open3.capture3("tmux", *args)
      raise Error, "tmux #{args.first} falhou: #{err}" unless status.success?
    end

    def shell_quote(str) = "'#{str.gsub("'", "'\\\\''")}'"
  end
end
