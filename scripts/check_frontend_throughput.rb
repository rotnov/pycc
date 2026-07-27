#!/usr/bin/env ruby
# frozen_string_literal: true

require "open3"

# Measures wall-clock time for `<pycc_bin> check <fixture>` and reports
# whether it stayed under `threshold_ms` -- an absolute floor (D-079), not a
# regression-vs-predecessor comparison like frontend-perf-gate's Criterion
# harness. A single measurement is sufficient here: this checks a fixed
# threshold, not two noisy measurements against each other.
def measure_and_check(pycc_bin, fixture_path, threshold_ms:)
  start = Process.clock_gettime(Process::CLOCK_MONOTONIC)
  _stdout, _stderr, status = Open3.capture3(pycc_bin, "check", fixture_path)
  elapsed_ms = (Process.clock_gettime(Process::CLOCK_MONOTONIC) - start) * 1000.0

  return { ok: false, elapsed_ms: elapsed_ms, reason: "pycc check exited non-zero" } unless status.success?
  return { ok: false, elapsed_ms: elapsed_ms, reason: "exceeded #{threshold_ms}ms threshold" } if elapsed_ms > threshold_ms

  { ok: true, elapsed_ms: elapsed_ms }
end

def main(arguments)
  if arguments.length < 2 || arguments.length > 3
    warn "usage: check_frontend_throughput.rb <pycc_bin> <fixture_path> [threshold_ms]"
    return 2
  end
  pycc_bin, fixture_path, threshold_arg = arguments
  threshold_ms = threshold_arg ? Float(threshold_arg) : 50.0
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
