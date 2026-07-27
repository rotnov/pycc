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
end
