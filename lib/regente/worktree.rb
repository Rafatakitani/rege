# frozen_string_literal: true

require "open3"
require "fileutils"
require "tmpdir"

module Regente
  # One isolated git worktree + branch per agent. Agents edit here in parallel
  # with zero file-level races; the reviewer diffs each branch.
  class Worktree
    attr_reader :repo, :name, :branch, :path

    def initialize(repo:, name:, branch_prefix: "regente", base: nil, root: nil)
      @repo = File.expand_path(repo)
      @name = name
      @branch = "#{branch_prefix}/#{name}"
      @base = base # base ref; nil => current HEAD
      @root = root || File.join(Dir.tmpdir, "regente-worktrees", File.basename(@repo))
      @path = File.join(@root, name)
    end

    def create
      FileUtils.mkdir_p(@root)
      base = @base || current_head
      git("worktree", "add", "-b", @branch, @path, base)
      @path
    end

    def commit_all(message)
      git_in(@path, "add", "-A")
      git_in(@path, "commit", "-q", "-m", message)
    end

    # Diff of this branch against the base ref it forked from.
    def diff
      base = @base || "HEAD"
      out, = git_capture("diff", "#{base}...#{@branch}")
      out
    end

    def remove(force: true)
      args = ["worktree", "remove"]
      args << "--force" if force
      args << @path
      git_capture(*args) # tolerate already-gone
      # best-effort branch cleanup
      git_capture("branch", "-D", @branch)
      FileUtils.rm_rf(@path)
    end

    def exist? = File.directory?(@path)

    def self.list(repo)
      out, = Open3.capture2("git", "-C", File.expand_path(repo), "worktree", "list", "--porcelain")
      out.scan(/^worktree (.+)$/).flatten
    end

    private

    def current_head
      out, = git_capture("rev-parse", "HEAD")
      out.strip
    end

    def git(*args)
      _out, err, status = git_capture(*args)
      raise Error, "git #{args.join(' ')} falhou: #{err}" unless status.success?
    end

    def git_capture(*args)
      Open3.capture3("git", "-C", @repo, *args)
    end

    def git_in(dir, *args)
      _o, err, status = Open3.capture3("git", "-C", dir, *args)
      raise Error, "git #{args.join(' ')} falhou: #{err}" unless status.success?
    end
  end
end
