# frozen_string_literal: true

module Regente
  # Named, opinionated color themes (DaisyUI-inspired) — not just dark/light.
  # Each theme is a palette of semantic roles as truecolor RGB triples plus a
  # shell-style prompt. Selected via config `ui.theme`.
  module Theme
    # role => [r, g, b]; :prompt is a string.
    PALETTES = {
      # matrix phosphor — the default hacker vibe
      "hacker" => {
        accent: [0, 255, 65], accent2: [0, 170, 60], dim: [70, 120, 80],
        text: [150, 255, 150], ok: [0, 255, 65], warn: [255, 200, 0],
        fail: [255, 60, 60], prompt: "root@regente:~# "
      },
      # gold on black
      "luxury" => {
        accent: [212, 175, 55], accent2: [160, 130, 60], dim: [120, 105, 70],
        text: [232, 220, 192], ok: [200, 170, 80], warn: [220, 160, 60],
        fail: [200, 70, 70], prompt: "❖ "
      },
      # neon magenta + cyan
      "cyberpunk" => {
        accent: [255, 42, 109], accent2: [5, 217, 232], dim: [120, 60, 90],
        text: [255, 220, 240], ok: [57, 255, 20], warn: [249, 240, 2],
        fail: [255, 42, 109], prompt: "▶ "
      },
      # sunset purple/pink/cyan
      "synthwave" => {
        accent: [255, 110, 199], accent2: [114, 239, 221], dim: [110, 90, 140],
        text: [240, 220, 255], ok: [114, 239, 221], warn: [255, 215, 120],
        fail: [255, 90, 140], prompt: "➤ "
      },
      # dracula
      "dracula" => {
        accent: [189, 147, 249], accent2: [255, 121, 198], dim: [98, 114, 164],
        text: [248, 248, 242], ok: [80, 250, 123], warn: [241, 250, 140],
        fail: [255, 85, 85], prompt: "λ "
      },
      # deep green
      "forest" => {
        accent: [88, 204, 120], accent2: [40, 140, 90], dim: [80, 110, 85],
        text: [220, 240, 220], ok: [88, 204, 120], warn: [220, 190, 80],
        fail: [230, 90, 80], prompt: "❯ "
      },
      # warm ember
      "ember" => {
        accent: [255, 122, 60], accent2: [255, 180, 80], dim: [130, 90, 70],
        text: [255, 225, 200], ok: [120, 200, 120], warn: [255, 180, 80],
        fail: [255, 80, 70], prompt: "❯ "
      }
    }.freeze

    DEFAULT = "hacker"

    module_function

    def names = PALETTES.keys

    def exist?(name) = PALETTES.key?(name)

    # Returns the palette hash for `name`, falling back to DEFAULT.
    def palette(name)
      PALETTES[name] || PALETTES[DEFAULT]
    end

    def color(name, role)
      palette(name)[role]
    end

    def prompt(name)
      palette(name)[:prompt]
    end
  end
end
