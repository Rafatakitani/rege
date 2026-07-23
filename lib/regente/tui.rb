# frozen_string_literal: true

require "tty/box"
require "reline"

module Regente
  # Full-screen terminal UI with a hacker-terminal feel and real named themes
  # (Regente::Theme). We render the master conversation natively (streamed),
  # instead of handing off to an external CLI's interface.
  class TUI
    def initialize(config:, repo:, out: $stdout, color: true,
                   launcher: nil, probe_runner: nil, exe: "regente", driver: nil)
      @config = config
      @repo = repo
      @out = out
      @color = color
      @theme = config.get("ui.theme") || Theme::DEFAULT
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
      bold(paint(:accent, logo))
    end

    def header_box
      master = "#{@config.master['cli']} · #{@config.master['model']}"
      lines = [
        "#{paint(:dim, 'mestre')}   #{bold(paint(:text, master))}",
        "#{paint(:dim, 'repo')}     #{bold(paint(:text, File.basename(@repo)))}",
        "#{paint(:dim, 'tema')}     #{paint(:accent2, @theme)}",
        "#{paint(:dim, 'workers')}  #{paint(:text, @config.workers.map { |w| w['cli'] }.join(', '))}"
      ]
      TTY::Box.frame(*lines, padding: [0, 1], title: { top_left: " regente " },
                             style: { border: { fg: :bright_black } })
    end

    def health_lines(results)
      results.map do |cli, ok|
        dot = ok ? paint(:ok, "●") : paint(:fail, "●")
        state = ok ? paint(:dim, "ok") : paint(:fail, "sem resposta")
        "  #{dot} #{cli.ljust(10)} #{state}"
      end.join("\n")
    end

    def footer
      keys = [["/theme", "trocar tema"], ["/doctor", "checar bots"],
              ["/config", "config"], ["/attach", "sessão externa"], ["/quit", "sair"]]
      paint(:dim, keys.map { |k, d| "#{k} #{d}" }.join("   ·   "))
    end

    def prompt_label = bold(paint(:accent, Theme.prompt(@theme)))

    def format_event(ev)
      case ev.type
      when :text then "#{paint(:accent, '●')} #{ev.text}"
      when :tool then paint(:dim, "  ⚙ #{ev.name} #{summarize(ev.input)}")
      when :done then ev.cost ? paint(:dim, format("  — $%.4f", ev.cost)) : nil
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
      when %r{\A/theme\b} then cmd_theme(line.split[1])
      when "/attach" then attach_external
      when "/help", "/?" then @out.puts(footer)
      when %r{\A/} then @out.puts(paint(:fail, "comando desconhecido: #{line}"))
      else chat(line)
      end
    end

    def cmd_theme(name)
      if name.nil?
        @out.puts(paint(:dim, "temas: #{Theme.names.join(', ')}  (atual: #{@theme})"))
      elsif Theme.exist?(name)
        @theme = name
        @config.set("ui.theme", name)
        @config.save_project
        draw_home
      else
        @out.puts(paint(:fail, "tema inexistente: #{name}"))
      end
    end

    def chat(text)
      @out.puts("\n#{prompt_label}#{text}\n")
      driver.send_turn(text) do |ev|
        line = format_event(ev)
        @out.puts(line) if line
      end
    rescue Regente::Error => e
      @out.puts(paint(:fail, e.message))
    end

    def driver
      @driver ||= MasterDriver.new(cli: @config.master["cli"], repo: @repo,
                                   prompt: Playbook.prompt(@config),
                                   model: @config.master["model"], exe: @exe)
    end

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
      @out.puts(paint(:dim, "  digite uma tarefa e o mestre orquestra — ou um comando:"))
      @out.puts(footer)
      @out.puts
    end

    def read_line
      Reline.readline("\n#{prompt_label}", true)
    rescue Interrupt
      nil
    end

    # ---- ANSI helpers ---------------------------------------------------

    def paint(role, text) = fg(Theme.color(@theme, role), text)

    def fg(rgb, text)
      return text unless @color && rgb

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
