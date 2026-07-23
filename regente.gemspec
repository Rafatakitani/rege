# frozen_string_literal: true

require_relative "lib/regente/version"

Gem::Specification.new do |spec|
  spec.name        = "regente"
  spec.version     = Regente::VERSION
  spec.authors     = ["Rafa"]
  spec.summary     = "Orquestrador multi-agente de IAs: um mestre comanda CLIs de IA isolados em git worktree + tmux."
  spec.description  = "TUI onde um modelo mestre (trocavel) comanda claude/codex/gemini/opencode " \
                     "headless via MCP, cada worker isolado em git worktree + tmux, revisa e abre PR."
  spec.homepage    = "https://github.com/takitani-labs/regente"
  spec.license     = "MIT"
  spec.required_ruby_version = ">= 3.2"

  spec.files = Dir["lib/**/*.rb", "exe/*", "README.md"]
  spec.bindir      = "exe"
  spec.executables = ["regente"]
  spec.require_paths = ["lib"]

  # runtime deps kept minimal on purpose; YAML/JSON/Open3 are stdlib
  spec.add_dependency "tty-prompt", "~> 0.23"
  spec.add_dependency "tty-box", "~> 0.7"
  spec.add_dependency "pastel", "~> 0.8"

  spec.add_development_dependency "minitest", "~> 5.20"
  spec.add_development_dependency "rake", "~> 13.0"
end
