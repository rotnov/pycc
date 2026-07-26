#!/usr/bin/env ruby
# frozen_string_literal: true

# Compares fixed five-replicate Criterion measurements for an exact
# predecessor/candidate pair. Each revision contributes every predeclared
# sample, and the unchanged 2% regression threshold is applied to the median
# of those five per-run medians whenever executable inputs changed. Identical
# executable inputs retain D-056's deterministic non-blocking telemetry path.
# A majority aggregate makes isolated hosted-runner spikes non-decisive without
# selecting or retrying samples after the result is known.

require "json"

REPLICATED_PAIRED_DEFAULT_THRESHOLD_PERCENT = 2.0
REPLICATED_PAIRED_SAMPLE_COUNT = 5
REPLICATED_PAIRED_SAMPLE_NAMES =
  (1..REPLICATED_PAIRED_SAMPLE_COUNT).map { |round| "round-#{round}.json" }.freeze

def replicated_median_point_estimate(path)
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

def replicated_median_estimates(directory)
  stat = File.lstat(directory)
  unless stat.directory? && !stat.symlink?
    raise "replicated timing path #{directory} must be a real directory"
  end

  actual_names = Dir.children(directory).sort
  unless actual_names == REPLICATED_PAIRED_SAMPLE_NAMES
    raise "replicated timing directory #{directory} must contain exactly " \
          "#{REPLICATED_PAIRED_SAMPLE_NAMES.join(', ')}"
  end

  REPLICATED_PAIRED_SAMPLE_NAMES.map do |name|
    path = File.join(directory, name)
    sample_stat = File.lstat(path)
    unless sample_stat.file? && !sample_stat.symlink?
      raise "replicated timing sample #{path} must be a regular file, not a symlink"
    end

    replicated_median_point_estimate(path)
  end
rescue Errno::ENOENT => e
  raise "could not read replicated timing directory #{directory}: #{e.message}"
end

def replicated_aggregate_median(estimates)
  sorted = estimates.sort
  sorted.fetch(sorted.length / 2)
end

def replicated_paired_regression_percent(current, previous)
  (current - previous) / previous * 100.0
end

def replicated_paired_main(arguments)
  current_dir, previous_dir, executable_inputs_equal_arg, threshold_arg =
    arguments
  unless current_dir && previous_dir && executable_inputs_equal_arg
    warn "usage: check_replicated_paired_perf_regression.rb <current_dir> <previous_dir> " \
         "<executable_inputs_equal:true|false> [threshold_percent]"
    return 2
  end
  unless %w[true false].include?(executable_inputs_equal_arg)
    raise ArgumentError, "executable_inputs_equal must be exactly true or false"
  end
  executable_inputs_equal = executable_inputs_equal_arg == "true"

  threshold =
    if threshold_arg
      Float(threshold_arg)
    else
      REPLICATED_PAIRED_DEFAULT_THRESHOLD_PERCENT
    end
  unless threshold.finite? && !threshold.negative?
    raise ArgumentError, "threshold_percent must be a finite non-negative number"
  end

  current_samples = replicated_median_estimates(current_dir)
  previous_samples = replicated_median_estimates(previous_dir)
  current = replicated_aggregate_median(current_samples)
  previous = replicated_aggregate_median(previous_samples)
  delta = replicated_paired_regression_percent(current, previous)

  puts "previous replicate medians: #{previous_samples.map { |value| format('%.2f', value) }.join(', ')} ns"
  puts "current replicate medians: #{current_samples.map { |value| format('%.2f', value) }.join(', ')} ns"
  puts format(
    "previous aggregate median: %.2f ns, current aggregate median: %.2f ns, " \
    "delta: %.4f%% (threshold: %.2f%%, executable inputs equal: %s)",
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
      "FAIL: pycc check replicated frontend median regressed %.4f%% (threshold: %.2f%%)",
      delta,
      threshold
    )
    return 1
  end

  puts "OK: fixed replicated measurement is within the regression threshold"
  0
rescue RuntimeError, ArgumentError => e
  warn e.message
  2
end

exit(replicated_paired_main(ARGV)) if $PROGRAM_NAME == __FILE__
