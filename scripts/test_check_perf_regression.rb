#!/usr/bin/env ruby
# frozen_string_literal: true

require "minitest/autorun"
require "tmpdir"
require "json"
require_relative "check_perf_regression"

class PerfRegressionTest < Minitest::Test
  def estimates_file(dir, name, mean_ns)
    path = File.join(dir, name)
    File.write(path, JSON.generate({ "mean" => { "point_estimate" => mean_ns } }))
    path
  end

  def test_exits_zero_when_within_threshold
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1010.0) # +1%, under the 2% default threshold
      assert_equal 0, main([current, previous])
    end
  end

  def test_exits_one_when_regression_exceeds_threshold
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1030.0) # +3%, over the 2% default threshold
      assert_equal 1, main([current, previous])
    end
  end

  def test_an_improvement_is_not_a_regression
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 500.0)
      assert_equal 0, main([current, previous])
    end
  end

  def test_respects_a_custom_threshold_argument
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1010.0) # +1%
      assert_equal 1, main([current, previous, "0.5"])
    end
  end

  def test_exits_two_on_missing_arguments
    assert_equal 2, main(["only_one_arg.json"])
  end

  def test_exits_two_when_a_file_is_missing
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      assert_equal 2, main([File.join(dir, "does_not_exist.json"), previous])
    end
  end
end
