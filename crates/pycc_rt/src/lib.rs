//! Runtime support packaged as a normal Cargo dependency.
//!
//! Generated object files call C-ABI functions from this runtime. The
//! target-independent C source is embedded in `pycc` through this crate and
//! compiled by the same compiler driver, for the same target, as the
//! generated object. This avoids a hidden dependency on a sibling
//! `target/debug` archive and keeps release builds, custom
//! `CARGO_TARGET_DIR` values, and cross-target builds on the same path.

use std::path::Path;

/// Minimal v0.1 runtime source.
///
/// `PRId64` is used instead of assuming that Rust's `i64` maps to C's
/// `long` on every Tier-1 ABI.
pub const C_RUNTIME_SOURCE: &str = r#"#include <inttypes.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#ifdef _WIN32
#include <fcntl.h>
#include <io.h>
#endif

static void pycc_rt_use_binary_stream(FILE *stream) {
#ifdef _WIN32
    if (_setmode(_fileno(stream), _O_BINARY) == -1) {
        exit(101);
    }
#else
    (void)stream;
#endif
}

void pycc_rt_print_i64(int64_t value) {
    pycc_rt_use_binary_stream(stdout);
    printf("%" PRId64 "\n", value);
}

void pycc_rt_name_error(const char *name) {
    pycc_rt_use_binary_stream(stderr);
    fprintf(stderr, "NameError: name '%s' is not defined\n", name);
    exit(101);
}
"#;

/// Materialize the embedded runtime beside a generated object before the
/// final compiler-driver invocation.
pub fn write_c_runtime(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, C_RUNTIME_SOURCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn materializes_the_embedded_runtime_source() {
        let dir = std::env::temp_dir().join(format!("pycc_rt_source_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("pycc_rt.c");
        write_c_runtime(&path).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), C_RUNTIME_SOURCE);
        assert!(C_RUNTIME_SOURCE.contains("exit(101);"));
        assert!(C_RUNTIME_SOURCE.contains("_setmode(_fileno(stream), _O_BINARY)"));
    }
}
