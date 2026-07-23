# frozen_string_literal: true

require "yaml"
require "fileutils"

module Regente
  # Layered configuration: DEFAULTS <- global (~/.config/regente/config.yml)
  # <- project (.regente.yml). Later layers deep-merge over earlier ones.
  #
  # Supports dot-path get/set and writing a layer back to disk so the TUI can
  # edit almost everything live.
  class Config
    DEFAULTS = {
      # The conversational master the user talks to. Swappable.
      "master" => { "cli" => "claude", "model" => "opus" },

      # role -> cli -> model mapping. Fully editable.
      "roster" => [
        { "role" => "triage",    "cli" => "claude",   "model" => "haiku" },
        { "role" => "planner",   "cli" => "claude",   "model" => "opus" },
        { "role" => "worker",    "cli" => "claude",   "model" => "sonnet" },
        { "role" => "worker",    "cli" => "codex",    "model" => nil },
        { "role" => "worker",    "cli" => "opencode", "model" => nil },
        { "role" => "reviewer",  "cli" => "claude",   "model" => "opus" },
        { "role" => "bughunter", "cli" => "claude",   "model" => "fable" }
      ],

      # per-role deadlines in seconds
      "timeouts" => {
        "triage" => 60,
        "worker" => 300,
        "reviewer" => 300,
        "healthcheck" => 15
      },

      "playbooks" => {
        "review_rounds" => 3, # max fix-loop rounds (hard mode)
        "retry_on_timeout" => 1
      },

      "pr" => {
        "provider" => "github", # github | gitlab | none
        "branch_prefix" => "regente"
      },

      "sandbox" => {
        "enabled" => true,
        "yolo" => true # auto-approve everything, zero prompts, confined to worktree
      },

      "ui" => {
        "theme" => "hacker" # see Regente::Theme::PALETTES
      }
    }.freeze

    attr_reader :data, :global_path, :project_path

    def self.load(project_dir:, home: Dir.home)
      global_path = File.join(home, ".config", "regente", "config.yml")
      project_path = project_dir ? File.join(project_dir, ".regente.yml") : nil

      merged = deep_dup(DEFAULTS)
      merged = deep_merge(merged, read_yaml(global_path))
      merged = deep_merge(merged, read_yaml(project_path)) if project_path

      new(merged, global_path: global_path, project_path: project_path)
    end

    def initialize(data, global_path: nil, project_path: nil)
      @data = data
      @global_path = global_path
      @project_path = project_path
      @project_overrides = self.class.read_yaml(project_path)
      @global_overrides = self.class.read_yaml(global_path)
    end

    def master   = @data["master"]
    def roster   = @data["roster"]
    def timeouts = @data["timeouts"]
    def playbooks = @data["playbooks"]
    def pr       = @data["pr"]
    def sandbox  = @data["sandbox"]

    def workers = roster.select { |r| r["role"] == "worker" }
    def role(name) = roster.find { |r| r["role"] == name }

    # dot-path read, e.g. get("master.cli")
    def get(path)
      path.split(".").reduce(@data) do |node, key|
        break nil unless node.is_a?(Hash)

        node[key]
      end
    end

    # dot-path write into the merged data AND the project override layer,
    # so save_project persists only what changed.
    def set(path, value)
      keys = path.split(".")
      assign(@data, keys, value)
      assign(@project_overrides, keys, value)
      value
    end

    def save_project
      raise Error, "no project path" unless @project_path

      File.write(@project_path, YAML.dump(@project_overrides))
    end

    def save_global
      raise Error, "no global path" unless @global_path

      FileUtils.mkdir_p(File.dirname(@global_path))
      File.write(@global_path, YAML.dump(@global_overrides))
    end

    class << self
      def read_yaml(path)
        return {} unless path && File.exist?(path)

        YAML.safe_load_file(path) || {}
      rescue Psych::SyntaxError => e
        raise Error, "config YAML invalido em #{path}: #{e.message}"
      end

      def deep_merge(base, over)
        return deep_dup(base) if over.nil? || over.empty?

        result = deep_dup(base)
        over.each do |k, v|
          result[k] = if result[k].is_a?(Hash) && v.is_a?(Hash)
                        deep_merge(result[k], v)
                      else
                        deep_dup(v)
                      end
        end
        result
      end

      def deep_dup(obj)
        case obj
        when Hash then obj.each_with_object({}) { |(k, v), h| h[k] = deep_dup(v) }
        when Array then obj.map { |e| deep_dup(e) }
        else obj
        end
      end
    end

    private

    def assign(hash, keys, value)
      *head, last = keys
      node = head.reduce(hash) { |n, k| n[k] ||= {} }
      node[last] = value
    end
  end
end
