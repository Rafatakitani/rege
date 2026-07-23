# frozen_string_literal: true

require_relative "regente/version"
require_relative "regente/config"
# Additional subsystems are required here as they are implemented:
%w[worktree tmux agent engine pr session].each do |mod|
  path = File.join(__dir__, "regente", "#{mod}.rb")
  require path if File.exist?(path)
end
%w[mcp/tools mcp/server playbook master tui cli].each do |mod|
  path = File.join(__dir__, "regente", "#{mod}.rb")
  require path if File.exist?(path)
end

# Regente: orquestrador multi-agente de IAs.
module Regente
  class Error < StandardError; end
end
