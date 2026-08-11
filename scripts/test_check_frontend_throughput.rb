require_relative "check_frontend_throughput"
require "minitest/autorun"
require "tmpdir"

class TestCheckFrontendThroughput < Minitest::Test
  def test_passes_when_pycc_check_is_fast_enough
    Dir.mktmpdir do |dir|
      fake_pycc = File.join(dir, "fake_pycc")
      File.write(fake_pycc, "#!/bin/sh\nexit 0\n")
      File.chmod(0o755, fake_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(fake_pycc, fixture, threshold_ms: 5000)
      assert result[:ok]
    end
  end

  def test_fails_when_pycc_check_exceeds_the_threshold
    Dir.mktmpdir do |dir|
      slow_pycc = File.join(dir, "slow_pycc")
      File.write(slow_pycc, "#!/bin/sh\nsleep 0.2\nexit 0\n")
      File.chmod(0o755, slow_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(slow_pycc, fixture, threshold_ms: 50)
      refute result[:ok]
    end
  end

  def test_median_ignores_a_single_slow_outlier_sample
    Dir.mktmpdir do |dir|
      flaky_pycc = File.join(dir, "flaky_pycc")
      counter = File.join(dir, "counter")
      File.write(counter, "0\n")
      File.write(flaky_pycc, <<~SCRIPT)
        #!/bin/sh
        n=$(cat "#{counter}")
        n=$((n + 1))
        echo "$n" > "#{counter}"
        if [ "$n" = "1" ]; then
          sleep 0.2
        fi
        exit 0
      SCRIPT
      File.chmod(0o755, flaky_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(flaky_pycc, fixture, threshold_ms: 50, samples: 3)
      assert result[:ok], "median of 3 samples should ignore the single slow outlier: #{result}"
    end
  end

  def test_fails_when_every_sample_exceeds_the_threshold
    Dir.mktmpdir do |dir|
      slow_pycc = File.join(dir, "slow_pycc")
      File.write(slow_pycc, "#!/bin/sh\nsleep 0.2\nexit 0\n")
      File.chmod(0o755, slow_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(slow_pycc, fixture, threshold_ms: 50, samples: 3)
      refute result[:ok]
      assert_includes result[:reason], "median of 3 runs"
    end
  end

  def test_raises_when_samples_is_not_a_positive_odd_integer
    Dir.mktmpdir do |dir|
      fake_pycc = File.join(dir, "fake_pycc")
      File.write(fake_pycc, "#!/bin/sh\nexit 0\n")
      File.chmod(0o755, fake_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_raises(ArgumentError) { measure_and_check(fake_pycc, fixture, threshold_ms: 50, samples: 2) }
      assert_raises(ArgumentError) { measure_and_check(fake_pycc, fixture, threshold_ms: 50, samples: 0) }
      assert_raises(ArgumentError) { measure_and_check(fake_pycc, fixture, threshold_ms: 50, samples: -1) }
    end
  end

  def test_fails_fast_when_any_sample_exits_non_zero
    Dir.mktmpdir do |dir|
      flaky_pycc = File.join(dir, "flaky_pycc")
      counter = File.join(dir, "counter")
      File.write(counter, "0\n")
      File.write(flaky_pycc, <<~SCRIPT)
        #!/bin/sh
        n=$(cat "#{counter}")
        n=$((n + 1))
        echo "$n" > "#{counter}"
        if [ "$n" = "2" ]; then
          exit 1
        fi
        exit 0
      SCRIPT
      File.chmod(0o755, flaky_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(flaky_pycc, fixture, threshold_ms: 5000, samples: 3)
      refute result[:ok]
      assert_equal "pycc check exited non-zero", result[:reason]
    end
  end

  def test_fails_when_pycc_check_itself_fails
    Dir.mktmpdir do |dir|
      broken_pycc = File.join(dir, "broken_pycc")
      File.write(broken_pycc, "#!/bin/sh\nexit 1\n")
      File.chmod(0o755, broken_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      result = measure_and_check(broken_pycc, fixture, threshold_ms: 5000)
      refute result[:ok]
    end
  end

  def test_main_returns_2_for_wrong_argument_count
    assert_equal 2, main([])
    assert_equal 2, main(["only_one_arg"])
    assert_equal 2, main(%w[bin fixture 50 extra])
  end

  def test_main_returns_0_when_pycc_check_is_fast_enough
    Dir.mktmpdir do |dir|
      fake_pycc = File.join(dir, "fake_pycc")
      File.write(fake_pycc, "#!/bin/sh\nexit 0\n")
      File.chmod(0o755, fake_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_equal 0, main([fake_pycc, fixture, "5000"])
    end
  end

  def test_main_defaults_to_a_75ms_threshold_when_omitted
    # A deliberately slow binary makes this deterministic: any measured
    # elapsed time comfortably exceeds 75ms, so the failure message only
    # names "75.0ms" if the omitted third argument really did default to
    # it (rather than, say, silently defaulting to 0 or some other value).
    Dir.mktmpdir do |dir|
      slow_pycc = File.join(dir, "slow_pycc")
      File.write(slow_pycc, "#!/bin/sh\nsleep 0.2\nexit 0\n")
      File.chmod(0o755, slow_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      _stdout, stderr = capture_io { assert_equal 1, main([slow_pycc, fixture]) }
      assert_includes stderr, "exceeded 75.0ms threshold"
    end
  end

  def test_main_returns_1_when_pycc_check_exceeds_the_threshold
    Dir.mktmpdir do |dir|
      slow_pycc = File.join(dir, "slow_pycc")
      File.write(slow_pycc, "#!/bin/sh\nsleep 0.2\nexit 0\n")
      File.chmod(0o755, slow_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_equal 1, main([slow_pycc, fixture, "50"])
    end
  end

  def test_main_returns_1_when_pycc_check_itself_fails
    Dir.mktmpdir do |dir|
      broken_pycc = File.join(dir, "broken_pycc")
      File.write(broken_pycc, "#!/bin/sh\nexit 1\n")
      File.chmod(0o755, broken_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_equal 1, main([broken_pycc, fixture, "5000"])
    end
  end

  def test_main_returns_2_for_a_non_numeric_threshold
    Dir.mktmpdir do |dir|
      fake_pycc = File.join(dir, "fake_pycc")
      File.write(fake_pycc, "#!/bin/sh\nexit 0\n")
      File.chmod(0o755, fake_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_equal 2, main([fake_pycc, fixture, "abc"])
    end
  end

  def test_main_returns_2_for_a_negative_threshold
    Dir.mktmpdir do |dir|
      fake_pycc = File.join(dir, "fake_pycc")
      File.write(fake_pycc, "#!/bin/sh\nexit 0\n")
      File.chmod(0o755, fake_pycc)
      fixture = File.join(dir, "fixture.py")
      File.write(fixture, "x = 1\n")
      assert_equal 2, main([fake_pycc, fixture, "-1"])
    end
  end
end
