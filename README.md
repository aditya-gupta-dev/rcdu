# rcdu

A Linux disk usage browser written in Rust, with ncdu 2.9.2-compatible JSON and indexed binary files. It measures allocated space and apparent sizes, handles hardlinks, and supports multithreaded scans, exclusions, refresh, shell launch and confirmed deletion.

```sh
scripts/cargo build --locked --release
target/release/rcdu -t 4 /path/to/directory
```

Builds use half the CPUs available through affinity and cgroup quotas, minimum one. Scan workers are independent: `-t 1` is the default, `-t 0` selects available CPUs. Linux, Rust 1.85+, Python 3, a C compiler and standard build tools are required. Zstandard is built by Cargo; ncurses is not required. The current glibc build needs glibc 2.28 or later for optional statx support. The tested host/toolchain is recorded in [measurement evidence](docs/measurement.md).

```sh
# Save a low-memory indexed scan; browse it without loading the whole tree.
target/release/rcdu --ignore-config -t 4 -O usage.ncdu /path/to/directory
target/release/rcdu -f usage.ncdu

# Single-worker streaming JSON, optionally compressed with Zstandard.
target/release/rcdu -t 1 -o usage.json /path/to/directory
target/release/rcdu -c -o usage.json.zst /path/to/directory

# Complete model scan without opening a terminal.
target/release/rcdu --ignore-config -0 --quit-after-scan -t 4 /path/to/directory
```

Arrow keys or `hjkl` navigate; `?` shows help, `i` opens details, `r` refreshes, `d` deletes after confirmation, and `b` opens a shell. `-r` disables deletion and `-rr` also disables shell launch. Imported files disable filesystem actions by default. Indexed imports always disable deletion and refresh. JSON stdin and export stdout use `-`; binary input requires a seekable file. Full UI opens `/dev/tty`, allowing redirected stdin/stdout.

See [CLI and configuration](docs/cli.md), [contributing](CONTRIBUTING.md), [compatibility and checks](docs/compatibility.md), [design decisions](docs/decisions.md), and [performance evidence](docs/measurement.md). Original rewrite requirements and the complete Zig function guide live in [docs/INSTRUCTIONS.md](docs/INSTRUCTIONS.md) and [docs/GUIDE.md](docs/GUIDE.md). The unmodified ncdu source/manual/build recipe remains in [reference/ncdu-2.9.2](reference/ncdu-2.9.2).

The reported 30-second/77-MB performance target remains unverified without the original workload and command. Synthetic results are recorded with raw observations; they are not evidence of meeting that target.

MIT license, with Martin Pool's natural-sort notice retained separately. See [credits](docs/credits.md).
