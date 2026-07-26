#!/usr/bin/env ruby
# frozen_string_literal: true

require "json"
require "minitest/autorun"
require "tmpdir"
require_relative "check_paired_perf_regression"

class PairedPerfRegressionTest < Minitest::Test
  def estimates_file(dir, name, median_ns)
    path = File.join(dir, name)
    File.write(
      path,
      JSON.generate({ "median" => { "point_estimate" => median_ns } })
    )
    path
  end

  def test_exits_zero_when_within_threshold
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1010.0)
      assert_equal 0, paired_main([current, previous])
    end
  end

  def test_exits_one_when_regression_exceeds_threshold
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1030.0)
      assert_equal 1, paired_main([current, previous])
    end
  end

  def test_an_improvement_is_not_a_regression
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 500.0)
      assert_equal 0, paired_main([current, previous])
    end
  end

  def test_respects_a_custom_threshold_argument
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1010.0)
      assert_equal 1, paired_main([current, previous, "0.5"])
    end
  end

  def test_exits_two_on_missing_arguments
    assert_equal 2, paired_main(["only_one_arg.json"])
  end

  def test_exits_two_when_a_file_is_missing
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      missing = File.join(dir, "does_not_exist.json")
      assert_equal 2, paired_main([missing, previous])
    end
  end

  def test_exits_two_when_json_is_malformed
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = File.join(dir, "current.json")
      File.write(current, "{")
      assert_equal 2, paired_main([current, previous])
    end
  end

  def test_exits_two_when_the_median_is_missing
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = File.join(dir, "current.json")
      File.write(current, JSON.generate({ "mean" => { "point_estimate" => 1000.0 } }))
      assert_equal 2, paired_main([current, previous])
    end
  end

  def test_exits_two_when_the_median_has_the_wrong_shape
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)

      [nil, [], "fast"].each_with_index do |median, index|
        current = File.join(dir, "current-#{index}.json")
        File.write(current, JSON.generate({ "median" => median }))
        assert_equal 2, paired_main([current, previous])
      end
    end
  end

  def test_exits_two_when_the_json_root_has_the_wrong_shape
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)

      [nil, [], "fast"].each_with_index do |root, index|
        current = File.join(dir, "current-root-#{index}.json")
        File.write(current, JSON.generate(root))
        assert_equal 2, paired_main([current, previous])
      end
    end
  end

  def test_exits_two_when_an_estimate_is_not_positive
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 0.0)
      current = estimates_file(dir, "current.json", 1000.0)
      assert_equal 2, paired_main([current, previous])
    end
  end

  def test_exits_two_when_an_estimate_is_not_numeric
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", "fast")
      current = estimates_file(dir, "current.json", 1000.0)
      assert_equal 2, paired_main([current, previous])
    end
  end

  def test_exits_two_when_threshold_is_negative_or_non_finite
    Dir.mktmpdir do |dir|
      previous = estimates_file(dir, "previous.json", 1000.0)
      current = estimates_file(dir, "current.json", 1000.0)
      assert_equal 2, paired_main([current, previous, "-1"])
      assert_equal 2, paired_main([current, previous, "NaN"])
    end
  end
end
