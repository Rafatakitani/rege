# frozen_string_literal: true

require "test_helper"

class WorktreeTest < Minitest::Test
  include TestHelpers

  def test_create_makes_isolated_branch_and_dir
    with_temp_repo do |repo|
      wt = Regente::Worktree.new(repo: repo, name: "alpha")
      path = wt.create
      assert File.directory?(path)
      assert File.exist?(File.join(path, "README.md")) # inherited from base
      assert_equal "regente/alpha", wt.branch
    ensure
      wt&.remove
    end
  end

  def test_commit_all_isolated_from_main
    with_temp_repo do |repo|
      wt = Regente::Worktree.new(repo: repo, name: "beta")
      wt.create
      File.write(File.join(wt.path, "new.txt"), "hello")
      wt.commit_all("add new.txt")

      # main working tree does NOT have the file
      refute File.exist?(File.join(repo, "new.txt"))
      # but the worktree branch does
      log = `git -C #{repo} log regente/beta --oneline`
      assert_includes log, "add new.txt"
    ensure
      wt&.remove
    end
  end

  def test_diff_shows_changes
    with_temp_repo do |repo|
      wt = Regente::Worktree.new(repo: repo, name: "gamma")
      wt.create
      File.write(File.join(wt.path, "new.txt"), "hello")
      wt.commit_all("work")
      diff = wt.diff
      assert_includes diff, "new.txt"
      assert_includes diff, "hello"
    ensure
      wt&.remove
    end
  end

  def test_remove_cleans_up
    with_temp_repo do |repo|
      wt = Regente::Worktree.new(repo: repo, name: "delta")
      path = wt.create
      wt.remove
      refute File.directory?(path)
    end
  end
end
