#!/usr/bin/env ruby
# frozen_string_literal: true

# Compares the median point estimates from a predecessor/candidate Criterion
# pair measured sequentially on one runner. When the reviewed workflow proves
# that every executable benchmark input is identical, the observed delta is
# environmental telemetry rather than evidence of a compiler regression. Any
# executable-input change retains the hard 2% regression threshold.

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
  current_path, previous_path, executable_inputs_equal_arg, threshold_arg =
    arguments
  unless current_path && previous_path && executable_inputs_equal_arg
    warn "usage: check_source_aware_perf_regression.rb <current_estimates.json> <previous_estimates.json> " \
         "<executable_inputs_equal:true|false> [threshold_percent]"
    return 2
  end
  unless %w[true false].include?(executable_inputs_equal_arg)
    raise ArgumentError, "executable_inputs_equal must be exactly true or false"
  end
  executable_inputs_equal = executable_inputs_equal_arg == "true"
  threshold =
    threshold_arg ? Float(threshold_arg) : PAIRED_DEFAULT_THRESHOLD_PERCENT
  unless threshold.finite? && !threshold.negative?
    raise ArgumentError, "threshold_percent must be a finite non-negative number"
  end

  current = median_point_estimate(current_path)
  previous = median_point_estimate(previous_path)
  delta = paired_regression_percent(current, previous)

  puts format(
    "previous median: %.2f ns, current median: %.2f ns, delta: %.2f%% " \
    "(threshold: %.2f%%, executable inputs equal: %s)",
    previous,
    current,
    delta,
    threshold,
    executable_inputs_equal
  )

  if executable_inputs_equal
    puts "OK: executable benchmark inputs are identical; timing delta is non-blocking environment telemetry"
    return 0
  end

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
