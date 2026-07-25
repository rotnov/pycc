#!/usr/bin/env ruby
# frozen_string_literal: true

# Compares two criterion `estimates.json` files (one per baseline directory,
# e.g. `target/criterion/<bench>/current/estimates.json` and
# `.../previous/estimates.json`) and fails if the current run's mean point
# estimate regressed by more than a threshold percentage against the
# previous run -- criterion's own CLI does not set a non-zero exit code on a
# detected regression (verified directly against criterion 0.8.2's source:
# its only `std::process::exit(1)` calls are for CLI-argument errors, not
# for `ComparisonResult::Regressed`, which only affects the printed report
# text), so this script is what actually gives DELIVERY_PLAN.md's ">2%
# regression fails the PR" wording real teeth.

require "json"

DEFAULT_THRESHOLD_PERCENT = 2.0

def mean_point_estimate(path)
  data = JSON.parse(File.read(path))
  estimate = data.fetch("mean").fetch("point_estimate")
  unless estimate.is_a?(Numeric) && estimate.finite? && estimate.positive?
    raise "mean point estimate in #{path} must be a finite positive number"
  end

  estimate.to_f
rescue Errno::ENOENT, JSON::ParserError, KeyError => e
  raise "could not read a mean point estimate from #{path}: #{e.message}"
end

def regression_percent(current, previous)
  (current - previous) / previous * 100.0
end

def main(arguments)
  current_path, previous_path, threshold_arg = arguments
  unless current_path && previous_path
    warn "usage: check_perf_regression.rb <current_estimates.json> <previous_estimates.json> [threshold_percent]"
    return 2
  end
  threshold = threshold_arg ? Float(threshold_arg) : DEFAULT_THRESHOLD_PERCENT
  unless threshold.finite? && !threshold.negative?
    raise ArgumentError, "threshold_percent must be a finite non-negative number"
  end

  current = mean_point_estimate(current_path)
  previous = mean_point_estimate(previous_path)
  delta = regression_percent(current, previous)

  puts format("previous: %.2f ns, current: %.2f ns, delta: %.2f%% (threshold: %.2f%%)", previous, current, delta,
              threshold)

  if delta > threshold
    warn format("FAIL: pycc check frontend timing regressed %.2f%% (threshold: %.2f%%)", delta, threshold)
    return 1
  end

  puts "OK: within the regression threshold"
  0
rescue RuntimeError, ArgumentError => e
  warn e.message
  2
end

exit(main(ARGV)) if $PROGRAM_NAME == __FILE__
