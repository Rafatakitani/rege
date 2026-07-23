# frozen_string_literal: true

require "test_helper"

class PRTest < Minitest::Test
  include TestHelpers

  def cfg(repo)
    Regente::Config.load(project_dir: repo, home: repo)
  end

  # make a branch regente/x with one commit, return to base
  def make_branch(repo, name: "regente/x", file: "feature.rb")
    base = `git -C #{repo} rev-parse --abbrev-ref HEAD`.strip
    system("git", "-C", repo, "checkout", "-q", "-b", name, exception: true)
    File.write(File.join(repo, file), "puts 'hi'\n")
    system("git", "-C", repo, "add", "-A", exception: true)
    system("git", "-C", repo, "commit", "-q", "-m", "work", exception: true)
    system("git", "-C", repo, "checkout", "-q", base, exception: true)
    name
  end

  def test_patch_fallback_when_no_remote_or_gh
    with_temp_repo do |repo|
      branch = make_branch(repo)
      pr = Regente::PR.new(repo: repo, config: cfg(repo), gh_available: false)
      result = pr.open(branch: branch, title: "t", body: "b")
      assert_equal :patch, result.mode
      assert File.exist?(result.ref)
      assert_includes File.read(result.ref), "feature.rb"
    end
  end

  def test_uses_gh_when_available_and_remote
    with_temp_repo do |repo|
      branch = make_branch(repo)
      system("git", "-C", repo, "remote", "add", "origin",
             "https://github.com/x/y.git", exception: true)
      captured = {}
      runner = lambda do |branch:, title:, body:|
        captured = { branch: branch, title: title, body: body }
        "https://github.com/x/y/pull/42\n"
      end
      pr = Regente::PR.new(repo: repo, config: cfg(repo),
                           gh_available: true, gh_runner: runner)
      result = pr.open(branch: branch, title: "my title", body: "my body")
      assert_equal :pr, result.mode
      assert_equal "https://github.com/x/y/pull/42", result.ref
      assert_equal "my title", captured[:title]
    end
  end

  def test_provider_none_forces_patch
    with_temp_repo do |repo|
      branch = make_branch(repo)
      c = cfg(repo)
      c.set("pr.provider", "none")
      pr = Regente::PR.new(repo: repo, config: c, gh_available: true)
      result = pr.open(branch: branch, title: "t", body: "b")
      assert_equal :patch, result.mode
    end
  end

  def test_build_body_includes_agent_trail
    body = Regente::PR.build_body(
      summary: "Corrigi o login.",
      agents: [{ name: "w1", cli: "claude", model: "opus", state: :done },
               { name: "w2", cli: "codex", model: nil, state: :failed }]
    )
    assert_includes body, "Corrigi o login."
    assert_includes body, "**w1** — claude (opus) — done"
    assert_includes body, "**w2** — codex — failed"
  end
end
