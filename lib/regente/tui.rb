# frozen_string_literal: true

require "tty/box"
require "reline"

module Regente
  # Full-screen terminal UI (alternate screen, like Claude Code). Shows a
  # branded cockpit — master, repo, roster health — and takes a task, then
  # hands off to the master session (tmux). Rendering is pure (testable);
  # the interactive loop wraps it with the alternate screen + Reline input.
  class TUI
    ACCENT = [255, 122, 60].freeze   # dispatch orange
    DIMC   = [115, 123, 134].freeze  # neutral
    OKC    = [62, 199, 126].freeze   # green
    FAILC  = [255, 107, 120].freeze  # red

    def initialize(config:, repo:, out: $stdout, color: true,
                   launcher: nil, probe_runner: nil, exe: "regente", driver: nil)
      @config = config
      @repo = repo
      @out = out
      @color = color
      @launcher = launcher || ->(cmd, cwd) { system(*cmd, chdir: cwd) }
      @probe_runner = probe_runner
      @exe = exe
      @driver = driver
    end

    # ---- pure renderers -------------------------------------------------

    def banner
      logo = <<~LOGO.chomp
        ██████  ███████  ██████  ███████ ███    ██ ████████ ███████
        ██   ██ ██      ██       ██      ████   ██    ██    ██
        ██████  █████   ██   ███ █████   ██ ██  ██    ██    █████
        ██   ██ ██      ██    ██ ██      ██  ██ ██    ██    ██
        ██   ██ ███████  ██████  ███████ ██   ████    ██    ███████
      LOGO
      bold(fg(ACCENT, logo))
    end

    def header_box
      master = "#{@config.master['cli']} · #{@config.master['model']}"
      lines = [
        "#{fg(DIMC, 'mestre')}   #{bold(master)}",
        "#{fg(DIMC, 'repo')}     #{bold(File.basename(@repo))}",
        "#{fg(DIMC, 'workers')}  #{@config.workers.map { |w| w['cli'] }.join(', ')}"
      ]
      TTY::Box.frame(*lines, padding: [0, 1], title: { top_left: " regente " },
                             style: { border: { fg: :bright_black } })
    end

    def health_lines(results)
      results.map do |cli, ok|
        dot = ok ? fg(OKC, "●") : fg(FAILC, "●")
        state = ok ? fg(DIMC, "ok") : fg(FAILC, "sem resposta")
        "  #{dot} #{cli.ljust(10)} #{state}"
      end.join("\n")
    end

    def footer
      keys = [["/doctor", "checar bots"], ["/config", "ver config"],
              ["/attach", "sessão do mestre"], ["/quit", "sair"]]
      fg(DIMC, keys.map { |k, d| "#{k} #{d}" }.join("   ·   "))
    end

    def prompt_label = bold(fg(ACCENT, "› "))

    # Render a streamed master event to a line (nil = don't print).
    def format_event(ev)
      case ev.type
      when :text then "#{fg(ACCENT, '●')} #{ev.text}"
      when :tool then fg(DIMC, "  ⚙ #{ev.name} #{summarize(ev.input)}")
      when :done then ev.cost ? fg(DIMC, format("  — $%.4f", ev.cost)) : nil
      end
    end

    def summarize(input)
      return "" unless input.is_a?(Hash)

      input.map { |k, v| "#{k}=#{truncate(v.to_s)}" }.join(" ")
    end

    def truncate(str, max = 40)
      str.length > max ? "#{str[0, max]}…" : str
    end

    # ---- interactive loop ----------------------------------------------

    def run
      enter_screen
      draw_home
      loop do
        line = read_line
        break if line.nil?

        line = line.strip
        next if line.empty?
        break if %w[/quit /q exit].include?(line)

        dispatch(line)
      end
    ensure
      leave_screen
    end

    private

    def dispatch(line)
      case line
      when "/doctor" then show_health
      when "/config" then @out.puts(@config.data.to_yaml)
      when "/attach" then attach_external
      when "/help", "/?" then @out.puts(footer)
      when %r{\A/} then @out.puts(fg(FAILC, "comando desconhecido: #{line}"))
      else chat(line) # a task -> talk to the master, streamed into our TUI
      end
    end

    # Send a turn to the master and render the streamed reply natively.
    def chat(text)
      @out.puts("\n#{prompt_label}#{text}\n")
      driver.send_turn(text) do |ev|
        line = format_event(ev)
        @out.puts(line) if line
      end
    rescue Regente::Error => e
      @out.puts(fg(FAILC, e.message))
    end

    def driver
      @driver ||= MasterDriver.new(cli: @config.master["cli"], repo: @repo,
                                   prompt: Playbook.prompt(@config),
                                   model: @config.master["model"], exe: @exe)
    end

    # Fallback: open the master's own CLI in tmux (non-claude masters).
    def attach_external
      m = @config.master
      cmd = Master.launch_string(cli: m["cli"], repo: @repo,
                                 prompt: Playbook.prompt(@config),
                                 model: m["model"], exe: @exe)
      leave_screen
      @launcher.call(["tmux", "new-session", "-A", "-s", CLI::MASTER_SESSION,
                      "-c", @repo, cmd], @repo)
      enter_screen
      draw_home
    end

    def show_health
      engine = Engine.new(repo: @repo, config: @config, probe_runner: @probe_runner)
      @out.puts(health_lines(engine.health_check))
    end

    def draw_home
      clear
      @out.puts(banner)
      @out.puts
      @out.puts(header_box)
      @out.puts(fg(DIMC, "  digite uma tarefa e o mestre orquestra — ou um comando:"))
      @out.puts(footer)
      @out.puts
    end

    def read_line
      Reline.readline("\n#{prompt_label}", true)
    rescue Interrupt
      nil
    end

    # ---- ANSI helpers ---------------------------------------------------

    def fg(rgb, text)
      return text unless @color

      "\e[38;2;#{rgb[0]};#{rgb[1]};#{rgb[2]}m#{text}\e[0m"
    end

    def bold(text)
      return text unless @color

      "\e[1m#{text}\e[0m"
    end

    def enter_screen = @out.print("\e[?1049h\e[H")
    def leave_screen = @out.print("\e[?1049l")
    def clear = @out.print("\e[2J\e[H")
  end
end
