# frozen_string_literal: true

require "test_helper"

class DashboardTest < Minitest::Test
  def with_logdir
    Dir.mktmpdir("regente-dash") { |d| yield d }
  end

  def test_lists_only_workers_excluding_master
    with_logdir do |dir|
      lister = -> { ["regente-master", "regente-a1", "regente-a2", "other"] }
      d = Regente::Dashboard.new(lister: lister, logdir: dir)
      names = d.workers.map(&:name)
      assert_equal %w[a1 a2], names
    end
  end

  def test_state_done_from_exit_zero
    with_logdir do |dir|
      File.write(File.join(dir, "regente-a1.log"), "trabalhando\n__RG_EXIT_0__\n")
      d = Regente::Dashboard.new(lister: -> { ["regente-a1"] }, logdir: dir)
      row = d.workers.first
      assert_equal :done, row.state
      assert_equal "trabalhando", row.last
    end
  end

  def test_state_failed_from_nonzero_exit
    with_logdir do |dir|
      File.write(File.join(dir, "regente-a1.log"), "boom\n__RG_EXIT_1__\n")
      d = Regente::Dashboard.new(lister: -> { ["regente-a1"] }, logdir: dir)
      assert_equal :failed, d.workers.first.state
    end
  end

  def test_state_unknown_when_no_log_and_not_alive
    with_logdir do |dir|
      d = Regente::Dashboard.new(lister: -> { ["regente-ghost"] }, logdir: dir)
      assert_equal :unknown, d.workers.first.state
    end
  end
end
