# frozen_string_literal: true

require "io/console"
require "tty/box"

module Regente
  # Full-screen terminal UI with a hacker-terminal feel and real named themes
  # (Regente::Theme). We render the master conversation natively (streamed),
  # instead of handing off to an external CLI's interface.
  class TUI
    STATE_ICON = { running: "◍", done: "●", failed: "✗", unknown: "○", idle: "○" }.freeze
    STATE_ROLE = { running: :warn, done: :ok, failed: :fail, unknown: :dim, idle: :dim }.freeze

    def initialize(config:, repo:, out: $stdout, color: true, launcher: nil,
                   probe_runner: nil, exe: "regente", driver: nil, dashboard: nil)
      @config = config
      @repo = repo
      @out = out
      @color = color
      @theme = config.get("ui.theme") || Theme::DEFAULT
      @launcher = launcher || ->(cmd, cwd) { system(*cmd, chdir: cwd) }
      @probe_runner = probe_runner
      @exe = exe
      @driver = driver
      @dashboard = dashboard
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
      keys = [["/agents", "status IAs"], ["/theme", "tema"], ["/doctor", "checar bots"],
              ["/config", "config"], ["/quit", "sair"]]
      paint(:dim, keys.map { |k, d| "#{k} #{d}" }.join("   ·   "))
    end

    def prompt_label = bold(paint(:accent, Theme.prompt(@theme)))

    # Bottom "dashboard" of live worker status (data from tmux, cross-process).
    def render_dashboard(rows = dashboard.workers)
      out = ["", paint(:dim, "  ── AGENTES ──────────────────────────────")]
      if rows.empty?
        out << paint(:dim, "     nenhum agente ativo")
      else
        rows.each do |r|
          icon = paint(STATE_ROLE.fetch(r.state, :dim), STATE_ICON.fetch(r.state, "○"))
          out << "  #{icon} #{paint(:text, r.name.ljust(8))} " \
                 "#{paint(STATE_ROLE.fetch(r.state, :dim), r.state.to_s.ljust(8))} " \
                 "#{paint(:dim, truncate(r.last, 44))}"
        end
      end
      out.join("\n")
    end

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

    # Full-screen loop: fixed regions (chat + pinned AGENTES dashboard + input),
    # raw single-key input, idle poll to refresh worker status. Blind-built —
    # the pure Screen.compose is what's tested.
    def run(input: $stdin)
      @in = input
      @chat = []
      @input = ""
      say(:info, "Regente pronto. Digite uma tarefa. /help pros comandos, /quit sai.")
      refresh_agents
      with_screen do
        redraw
        loop do
          key = read_key(0.6)
          if key.nil?
            refresh_agents
            redraw
            next
          end
          break if handle_key(key) == :quit
        end
      end
    end

    private

    def handle_key(key)
      case key
      when "", "" then :quit          # ctrl-c / ctrl-d
      when "\r", "\n" then submit
      when "", "\b" then @input = @input[0..-2].to_s; redraw; nil
      else
        @input += key if key =~ /[[:print:]]/
        redraw
        nil
      end
    end

    def submit
      line = @input.strip
      @input = ""
      redraw
      return nil if line.empty?
      return :quit if %w[/quit /q exit].include?(line)

      say(:user, "#{Theme.prompt(@theme)}#{line}")
      dispatch(line)
      nil
    end

    def dispatch(line)
      case line
      when "/doctor" then show_health
      when "/config" then @config.data.to_yaml.each_line { |l| push(:info, l.chomp) }; redraw
      when %r{\A/theme\b} then cmd_theme(line.split[1])
      when "/agents", "/ps" then refresh_agents; redraw
      when "/attach" then attach_external
      when "/help", "/?" then say(:info, "comandos: /agents /theme <t> /doctor /config /attach /quit")
      when %r{\A/} then say(:error, "comando desconhecido: #{line}")
      else chat(line)
      end
    end

    def cmd_theme(name)
      if name.nil?
        say(:info, "temas: #{Theme.names.join(', ')}  (atual: #{@theme})")
      elsif Theme.exist?(name)
        @theme = name
        @config.set("ui.theme", name)
        @config.save_project
        redraw
      else
        say(:error, "tema inexistente: #{name}")
      end
    end

    def chat(text)
      driver.send_turn(text) do |ev|
        push_event(ev)
        refresh_agents
        redraw
      end
    rescue Regente::Error => e
      say(:error, e.message)
    end

    def push_event(ev)
      case ev.type
      when :text then push(:assistant, "● #{ev.text}")
      when :tool then push(:tool, "  ⚙ #{ev.name} #{summarize(ev.input)}")
      when :done then push(:cost, format("  — $%.4f", ev.cost)) if ev.cost
      end
    end

    def driver
      @driver ||= MasterDriver.new(cli: @config.master["cli"], repo: @repo,
                                   prompt: Playbook.prompt(@config),
                                   model: @config.master["model"], exe: @exe)
    end

    def dashboard
      @dashboard ||= Dashboard.new
    end

    def refresh_agents
      @agents = dashboard.workers
    rescue StandardError
      @agents = []
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
      redraw
    end

    def show_health
      engine = Engine.new(repo: @repo, config: @config, probe_runner: @probe_runner)
      engine.health_check.each { |cli, ok| push(:info, "  #{ok ? '●' : '✗'} #{cli} #{ok ? 'ok' : 'sem resposta'}") }
      redraw
    end

    # ---- screen plumbing ----

    def say(role, text)
      push(role, text)
      redraw
    end

    def push(role, text)
      (@chat ||= []) << { role: role, text: text }
    end

    def redraw
      rows, cols = dims
      master = "#{@config.master['cli']} · #{@config.master['model']}"
      @out.print(Screen.compose(width: cols, height: rows, theme: @theme,
                                master: master, repo: @repo, chat: @chat || [],
                                agents: @agents || [], input: @input || "", color: @color))
    end

    def dims
      if @out.respond_to?(:winsize)
        @out.winsize
      else
        [24, 80]
      end
    rescue StandardError
      [24, 80]
    end

    def read_key(timeout)
      return nil unless IO.select([@in], nil, nil, timeout)

      @in.getc
    rescue IOError
      nil
    end

    def with_screen
      enter_screen
      @out.print("\e[?25l") # hide cursor
      raw = @in.respond_to?(:raw) && @in.tty?
      raw ? @in.raw { yield } : yield
    ensure
      @out.print("\e[?25h") # show cursor
      leave_screen
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
