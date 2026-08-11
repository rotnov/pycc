#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"

# Measures wall-clock time for `<pycc_bin> check <fixture>` and reports
# whether the median of `samples` runs stayed under `threshold_ms` -- an
# absolute floor (D-079), not a regression-vs-predecessor comparison like
# frontend-perf-gate's Criterion harness. D-132 moved this from a single
# wall-clock sample to a median-of-`samples` measurement: historical CI data
# (~209 sampled runs, D-132) showed the check normally lands at 15-32ms
# against the 75ms threshold with zero corroborating failures, so a lone
# 54.55ms spike was runner-noise on one draw, not a regression. `samples`
# must be odd so the median is a single element, not an average of two.
def measure_and_check(pycc_bin, fixture_path, threshold_ms:, samples: 3)
  raise ArgumentError, "samples must be a positive odd integer" unless samples.positive? && samples.odd?

  elapsed_times = []
  samples.times do
    start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
    _stdout, _stderr, status = Open3.capture3(pycc_bin, "check", fixture_path)
    elapsed_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - start) * 1000.0

    return { ok: false, elapsed_ms: elapsed_ms, reason: "pycc check exited non-zero" } unless status.success?

    elapsed_times << elapsed_ms
  end

  median_ms = elapsed_times.sort[elapsed_times.length / 2]
  if median_ms > threshold_ms
    reason = "exceeded #{threshold_ms}ms threshold (median of #{samples} runs: #{elapsed_times.sort})"
    return { ok: false, elapsed_ms: median_ms, reason: reason }
  end

  { ok: true, elapsed_ms: median_ms }
end

def main(arguments)
  if arguments.length < 2 || arguments.length > 3
    warn "usage: check_frontend_throughput.rb <pycc_bin> <fixture_path> [threshold_ms]"
    return 2
  end
  pycc_bin, fixture_path, threshold_arg = arguments
  threshold_ms = threshold_arg ? Float(threshold_arg) : 75.0
  unless threshold_ms.finite? && !threshold_ms.negative?
    raise ArgumentError, "threshold_ms must be a finite non-negative number"
  end

  result = measure_and_check(pycc_bin, fixture_path, threshold_ms: threshold_ms)
  if result[:ok]
    puts "OK: pycc check took #{result[:elapsed_ms].round(2)}ms (threshold #{threshold_ms}ms)"
    0
  else
    warn "FAIL: #{result[:reason]} (measured #{result[:elapsed_ms].round(2)}ms)"
    1
  end
rescue ArgumentError => e
  warn e.message
  2
end

exit(main(ARGV)) if __FILE__ == $PROGRAM_NAME
