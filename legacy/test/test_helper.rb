# frozen_string_literal: true

$LOAD_PATH.unshift File.expand_path("../lib", __dir__)

require "minitest/autorun"
require "tmpdir"
require "fileutils"
require "regente"

module TestHelpers
  # Create a throwaway git repo and yield its path.
  def with_temp_repo
    Dir.mktmpdir("regente-test") do |dir|
      system("git", "-C", dir, "init", "-q", exception: true)
      system("git", "-C", dir, "config", "user.email", "test@test.local", exception: true)
      system("git", "-C", dir, "config", "user.name", "test", exception: true)
      File.write(File.join(dir, "README.md"), "# test\n")
      system("git", "-C", dir, "add", "-A", exception: true)
      system("git", "-C", dir, "commit", "-q", "-m", "init", exception: true)
      yield dir
    end
  end
end
