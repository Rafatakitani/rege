# frozen_string_literal: true

require "open3"
require "fileutils"

module Regente
  # Turns a finished branch into a pull request. Never merges (Q06/Q16):
  # default opens a PR via `gh`; falls back to a local .patch file when there
  # is no GitHub remote / gh / when provider is "none".
  class PR
    Result = Struct.new(:mode, :ref, keyword_init: true)

    def initialize(repo:, config:, gh_runner: nil, gh_available: nil)
      @repo = File.expand_path(repo)
      @config = config
      @gh_runner = gh_runner || method(:default_gh_runner)
      @gh_available = gh_available
    end

    # branch: source branch, title/body: PR text (body usually written by the
    # master). Returns Result(mode: :pr|:patch, ref: url|path).
    def open(branch:, title:, body:)
      if github? && gh_available? && remote?
        url = @gh_runner.call(branch: branch, title: title, body: body)
        Result.new(mode: :pr, ref: url.to_s.strip)
      else
        Result.new(mode: :patch, ref: write_patch(branch))
      end
    end

    # Default PR body: what the master summarizes + the agent trail.
    def self.build_body(summary:, agents:)
      lines = [summary, "", "## Agentes", ""]
      agents.each do |a|
        model = a[:model] ? " (#{a[:model]})" : ""
        lines << "- **#{a[:name]}** — #{a[:cli]}#{model} — #{a[:state]}"
      end
      lines.join("\n")
    end

    private

    def github? = @config.pr["provider"] == "github"

    def gh_available?
      return @gh_available unless @gh_available.nil?

      system("which", "gh", out: File::NULL, err: File::NULL)
    end

    def remote?
      out, = Open3.capture2("git", "-C", @repo, "remote")
      !out.strip.empty?
    end

    def default_gh_runner(branch:, title:, body:)
      out, err, status = Open3.capture3(
        "gh", "pr", "create", "--head", branch, "--title", title, "--body", body,
        chdir: @repo
      )
      raise Error, "gh pr create falhou: #{err}" unless status.success?

      out
    end

    def write_patch(branch)
      base = default_branch
      out, err, status = Open3.capture3("git", "-C", @repo, "diff", "#{base}...#{branch}")
      raise Error, "git diff falhou: #{err}" unless status.success?

      dir = File.join(@repo, ".regente-runs")
      FileUtils.mkdir_p(dir)
      path = File.join(dir, "#{branch.tr('/', '-')}.patch")
      File.write(path, out)
      path
    end

    def default_branch
      out, = Open3.capture2("git", "-C", @repo, "rev-parse", "--abbrev-ref", "HEAD")
      out.strip
    end
  end
end
