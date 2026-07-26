#!/usr/bin/env ruby
# frozen_string_literal: true

# Compares the median point estimates from a predecessor/candidate Criterion
# pair measured sequentially on one runner. The median is deliberate: the
# paired D-051 validation observed identical Rust code whose means differed by
# 2.94% because one side contained many severe outliers, while the medians
# differed by only 0.56%. The hard regression threshold remains 2%.

require "json"

PAIRED_DEFAULT_THRESHOLD_PERCENT = 2.0

def median_point_estimate(path)
  data = JSON.parse(File.read(path))
  unless data.is_a?(Hash)
    raise "Criterion estimates in #{path} must be an object"
  end

  median = data.fetch("median")
  unless median.is_a?(Hash)
    raise "median estimate in #{path} must be an object"
  end

  estimate = median.fetch("point_estimate")
  unless estimate.is_a?(Numeric) && estimate.finite? && estimate.positive?
    raise "median point estimate in #{path} must be a finite positive number"
  end

  estimate.to_f
rescue Errno::ENOENT, JSON::ParserError, KeyError => e
  raise "could not read a median point estimate from #{path}: #{e.message}"
end

def paired_regression_percent(current, previous)
  (current - previous) / previous * 100.0
end

def paired_main(arguments)
  current_path, previous_path, threshold_arg = arguments
  unless current_path && previous_path
    warn "usage: check_paired_perf_regression.rb <current_estimates.json> <previous_estimates.json> " \
         "[threshold_percent]"
    return 2
  end
  threshold =
    threshold_arg ? Float(threshold_arg) : PAIRED_DEFAULT_THRESHOLD_PERCENT
  unless threshold.finite? && !threshold.negative?
    raise ArgumentError, "threshold_percent must be a finite non-negative number"
  end

  current = median_point_estimate(current_path)
  previous = median_point_estimate(previous_path)
  delta = paired_regression_percent(current, previous)

  puts format(
    "previous median: %.2f ns, current median: %.2f ns, delta: %.2f%% (threshold: %.2f%%)",
    previous,
    current,
    delta,
    threshold
  )

  if delta > threshold
    warn format(
      "FAIL: pycc check frontend median regressed %.2f%% (threshold: %.2f%%)",
      delta,
      threshold
    )
    return 1
  end

  puts "OK: within the regression threshold"
  0
rescue RuntimeError, ArgumentError => e
  warn e.message
  2
end

exit(paired_main(ARGV)) if $PROGRAM_NAME == __FILE__
