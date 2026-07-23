# frozen_string_literal: true

module Regente
  # Composes a full-screen frame with fixed regions: top bar, scrolling chat,
  # a pinned AGENTES dashboard at the bottom, and an input line. Pure — given
  # state + dimensions it returns the frame string. The TUI loop owns IO.
  module Screen
    ROLE_COLOR = { user: :accent, assistant: :text, tool: :dim, cost: :dim,
                   info: :dim, error: :fail }.freeze
    STATE_ICON = { running: "◍", done: "●", failed: "✗", unknown: "○", idle: "○" }.freeze
    STATE_COLOR = { running: :warn, done: :ok, failed: :fail, unknown: :dim, idle: :dim }.freeze
    MARGIN = 2

    module_function

    # chat: [{role:, text:}], agents: [Dashboard::Row], input: String
    def compose(width:, height:, theme:, master:, repo:, chat:, agents:, input:,
                color: true)
      inner = width - MARGIN
      dash_h = agents.empty? ? 1 : [[agents.size, 8].min, 1].max
      chat_h = [height - dash_h - 3, 1].max # topbar + agents title + input

      lines = []
      lines << topbar(theme, master, repo, inner, color)
      lines.concat(chat_region(chat, chat_h, inner, theme, color))
      lines << paint(theme, :dim, "── AGENTES " + ("─" * [inner - 11, 0].max), color)
      lines.concat(dash_region(agents, dash_h, inner, theme, color))
      lines << input_line(theme, input, color)

      # exactly `height` rows
      lines = lines.first(height)
      lines << "" while lines.size < height

      "\e[H" + lines.map { |l| pad(l) + "\e[K" }.join("\r\n")
    end

    def topbar(theme, master, repo, inner, color)
      brand = paint(theme, :accent, "▛▀ REGENTE", color)
      meta = paint(theme, :dim, truncate("#{theme} · #{master} · #{File.basename(repo)}", inner - 12), color)
      "#{brand}  #{meta}"
    end

    def chat_region(chat, rows, inner, theme, color)
      view = chat.last(rows)
      out = view.map do |e|
        paint(theme, ROLE_COLOR.fetch(e[:role], :text), truncate(e[:text], inner), color)
      end
      out.unshift("") while out.size < rows # push content to bottom
      out.first(rows)
    end

    def dash_region(agents, rows, inner, theme, color)
      if agents.empty?
        [paint(theme, :dim, "   nenhum agente ativo", color)]
      else
        agents.first(rows).map do |r|
          icon = STATE_ICON.fetch(r.state, "○")
          txt = "#{icon} #{r.name.to_s.ljust(8)} #{r.state.to_s.ljust(8)} #{r.last}"
          paint(theme, STATE_COLOR.fetch(r.state, :dim), truncate(txt, inner), color)
        end
      end
    end

    def input_line(theme, input, color)
      paint(theme, :accent, Theme.prompt(theme), color) + input + "█"
    end

    # ---- helpers ----

    def pad(line) = (" " * MARGIN) + line

    def truncate(str, max)
      return str if max <= 0 || str.length <= max

      "#{str[0, [max - 1, 0].max]}…"
    end

    def paint(theme, role, text, color)
      rgb = Theme.color(theme, role)
      return text unless color && rgb

      "\e[38;2;#{rgb[0]};#{rgb[1]};#{rgb[2]}m#{text}\e[0m"
    end
  end
end
