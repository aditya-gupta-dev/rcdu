# Current scan measurements

`scripts/benchmark` measures the live scanner across worker counts, using plain-text root totals for stability checks. It no longer exports or reimports trees or compares ncdu wire formats. Root totals are a limited check, not a per-entry oracle; scanner behavior is verified separately by Rust tests and disposable CLI/PTY fixtures. Benchmark driver JSON records timings only, and is not an application import/export format.

Build the native monitor with `mkdir -p target/tools` then `cc -O2 -Wall -Wextra -Werror scripts/measure.c -o target/tools/measure`. Example: `scripts/benchmark --root /path/to/tree --workers 1,4 --runs 5 --label live-scan`. Build `target/release/rcdu` first with `scripts/cargo build --release`.

The notes below describe historical measurements before compatibility removal.

# Measurement protocol

The synthetic broad fixture has 100,000 empty files and 1,024 directories. `benchmarks/workload.json` records generation. Timing includes the entire browser model and final directory/hardlink accounting, with `--ignore-config -0 --quit-after-scan`, matched workers, a warmup, five alternating-order observations, and byte-aware normalized export checks outside measurement. This fixture does not represent the user's 60 GB tree.

GNU time is not installed. `scripts/measure.c` uses Linux `wait4` child rusage and `CLOCK_MONOTONIC`, the same kernel RSS metric as GNU time. The native monitor's memory is excluded. Compile with `cc -O2 -Wall -Wextra -Werror scripts/measure.c -o target/tools/measure` (one compiler process). Sanity checks: `/bin/true` yielded 1,460 KiB, and a Python process allocating/touching 64 MiB yielded 76,980 KiB. Raw Linux `ru_maxrss` is KiB; decimal MB is KiB × 1024 / 1,000,000.

The first Python direct-child measurement was rejected: every child inherited a 43–50 MiB pre-exec high-water mark from the driver, concealing scanner memory. Its complete observations remain in `rust-initial-model.json`, marked invalid. Never use its RSS for a performance claim. `rust-model-valid.json` uses the native monitor instead.

The historical ncdu comparison used version 2.9.2. Exact distribution build flags are unknown; Rust initially used ordinary Cargo release with a portable target. It does not establish a fully controlled source-build comparison. No cache drops, root commands, remounting or user-tree changes were performed. Cache state was warm, not controlled cold.
