#!/usr/bin/env ruby
# frozen_string_literal: true

require "json"
require "minitest/autorun"
require "tmpdir"
require_relative "check_replicated_paired_perf_regression"

class ReplicatedPairedPerfRegressionTest < Minitest::Test
  def estimates_file(dir, round, median_ns)
    path = File.join(dir, "round-#{round}.json")
    File.write(
      path,
      JSON.generate({ "median" => { "point_estimate" => median_ns } })
    )
    path
  end

  def replicated_estimates(root, name, medians)
    dir = File.join(root, name)
    Dir.mkdir(dir)
    medians.each_with_index do |median, index|
      estimates_file(dir, index + 1, median)
    end
    dir
  end

  def test_exits_zero_when_the_fixed_sample_median_is_within_threshold
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000, 1_002, 998, 1_001, 999])
      current = replicated_estimates(root, "current", [1_010, 1_012, 1_008, 5_000, 1_009])

      assert_equal 0, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_one_when_a_majority_of_samples_exceeds_the_threshold
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000, 1_001, 999, 1_002, 998])
      current = replicated_estimates(root, "current", [1_010, 1_030, 1_031, 1_032, 900])

      assert_equal 1, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_identical_executable_inputs_make_any_delta_non_blocking
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [5_000] * 5)

      assert_equal 0, replicated_paired_main([current, previous, "true"])
    end
  end

  def test_exits_two_when_executable_input_identity_is_missing_or_invalid
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)

      assert_equal 2, replicated_paired_main([current, previous])
      assert_equal 2, replicated_paired_main([current, previous, "unknown"])
    end
  end

  def test_exactly_the_threshold_passes_but_any_greater_regression_fails
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_020] * 5)
      assert_equal 0, replicated_paired_main([current, previous, "false"])

      current = File.join(root, "current")
      5.times { |index| estimates_file(current, index + 1, 1_020.01) }
      assert_equal 1, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_an_improvement_is_not_a_regression
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [500] * 5)

      assert_equal 0, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_respects_a_custom_threshold_argument
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_010] * 5)

      assert_equal 1,
                   replicated_paired_main([current, previous, "false", "0.5"])
    end
  end

  def test_exits_two_on_missing_arguments
    assert_equal 2, replicated_paired_main(["only_one_dir"])
  end

  def test_exits_two_when_a_directory_is_missing
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)

      assert_equal 2,
                   replicated_paired_main(
                     [File.join(root, "missing"), previous, "false"]
                   )
    end
  end

  def test_exits_two_when_any_fixed_round_is_missing
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 4)

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_an_extra_sample_is_present
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)
      estimates_file(current, 6, 1_000)

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_a_sample_is_a_symlink
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)
      File.unlink(File.join(current, "round-5.json"))
      File.symlink(
        File.join(current, "round-4.json"),
        File.join(current, "round-5.json")
      )

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_json_is_malformed
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)
      File.write(File.join(current, "round-3.json"), "{")

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_the_median_is_missing_or_has_the_wrong_shape
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)

      File.write(
        File.join(current, "round-2.json"),
        JSON.generate({ "mean" => { "point_estimate" => 1_000 } })
      )
      assert_equal 2, replicated_paired_main([current, previous, "false"])

      File.write(
        File.join(current, "round-2.json"),
        JSON.generate({ "median" => [] })
      )
      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_the_json_root_has_the_wrong_shape
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)
      File.write(File.join(current, "round-2.json"), JSON.generate([]))

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_an_estimate_is_not_a_finite_positive_number
    [0, -1, "fast"].each do |invalid|
      Dir.mktmpdir do |root|
        previous = replicated_estimates(root, "previous", [1_000] * 5)
        current = replicated_estimates(root, "current", [1_000] * 5)
        estimates_file(current, 4, invalid)

        assert_equal 2, replicated_paired_main([current, previous, "false"])
      end
    end

    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)
      File.write(
        File.join(current, "round-4.json"),
        '{"median":{"point_estimate":1e400}}'
      )

      assert_equal 2, replicated_paired_main([current, previous, "false"])
    end
  end

  def test_exits_two_when_threshold_is_negative_or_non_finite
    Dir.mktmpdir do |root|
      previous = replicated_estimates(root, "previous", [1_000] * 5)
      current = replicated_estimates(root, "current", [1_000] * 5)

      assert_equal 2,
                   replicated_paired_main([current, previous, "false", "-1"])
      assert_equal 2,
                   replicated_paired_main([current, previous, "false", "NaN"])
    end
  end
end
