# Contributing

Use `scripts/cargo` for every Cargo command. It derives the effective CPU budget from Linux affinity and visible cgroup v1/v2 ancestor quotas, rounds fractional quota up, then halves the result. It also limits native build jobs and test threads. Explicit `-j` overrides are rejected. Do not run independent builds concurrently. Compilation settings do not set application scan workers.

```sh
scripts/check
scripts/cargo build --locked --release
scripts/cargo build --locked --profile profiling
```

`check` runs formatting, check, strict clippy, debug/release tests and attribution checks. If ncdu is installed, it also runs differential file-format and PTY tests on temporary fixtures. Run those tests explicitly against the reference executable with `scripts/integration --baseline target/baseline/ncdu-source`. Permission tests require an unprivileged user. Never point mutation tests at a user directory. `mandoc`/REUSE/Miri/perf checks are optional tools; report them as unavailable when absent, rather than substituting a fake pass.

The project is one library crate and a small executable. `session` chooses scan/import/export/browser modes. `config` and `cli` preserve byte arguments and config precedence. `scan` owns a fixed pool, bounded handoff queue, cooperative overflow traversal and cancellation. `model` owns compact worker arenas, byte-name storage, directory/extended/hardlink side tables and accounting. `format/json` is a byte-preserving streaming parser/writer; `sink` implements serial source completion; `format/binary_pool` streams parallel postorder blocks; `format/binary` validates and lazily reads EX1 blocks. `browser` owns navigation/sorting/action policy; `ui` owns safe display and a single-threaded terminal adapter. `delete` operates on known scanned children and rebuilds observations after changes. All Linux syscall, filesystem/account and terminal OS code belongs in `src/os/linux.rs` behind `os/mod.rs`.

Entry IDs encode worker and slot. They are stable for one model, not across replacement. Refresh builds a replacement transactionally, compacts reachable observations and restores browser position using raw component names. Never retain an old ID after refresh. Ordinary files own neither a full path nor an allocation per filename. `Model::path` reconstructs paths only when needed. Names stay bytes through filesystem and codecs; only display is sanitized. Allocated space is `blocks * 512` with saturation; apparent size is independent. Directory totals include their own sizes. A hardlink contributes once to every containing ancestor, with shared usage when links exist outside that ancestor.

To add a sort field, extend `config::SortField` and config parsing, add its comparison to `browser::compare`, wire its key/help if appropriate, and add a behavior case to `tests/display.rs`. Use differing values plus a name tie; verify display filtering does not change totals. This should require no syscall changes. To add an exclusion fixture, extend `tests/display.rs` for component matching or `tests/scan.rs` for traversal behavior. Place filesystem changes in the fixture's owned temporary directory and test both one and multiple workers. For compatibility-sensitive changes add a Rust/ncdu differential case in `scripts/integration`.

Unsafe code needs a SAFETY explanation covering ownership, initialization, lifetime, alignment, masks and bounds. Do not reinterpret packed records as references. Failed metadata calls are observations/errors, not proof of successful deletion. Only ENOENT proves disappearance for a custom command. Preserve no-follow directory traversal and default-No confirmation.

For performance changes, retain the previous release executable and run [the benchmark protocol](docs/measurement.md) against it and ncdu. Include model finalization, compare wall time and RSS together, and investigate changes over 5%. Keep all raw runs and noisy/rejected results with explanations. Generate fixtures/build artifacts outside timing. Use the portable CPU target for primary evidence; record native/PGO/LTO/allocator experiments separately.
