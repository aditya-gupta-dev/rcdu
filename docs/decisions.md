# Decision log

- Preserve ncdu 2.9.2 under `reference/ncdu-2.9.2/`, including notices, license, source, build recipe and manual. Its README/source links in GUIDE refer to this snapshot. The installed `/usr/bin/ncdu` is version 2.9.2; an independent executable copy lives in ignored `target/baseline/ncdu-2.9.2`.
- Linux only. All filesystem syscalls, account lookup and terminal OS operations are isolated in `src/os/linux.rs`. Serialization, model and browser policy remain portable.
- Keep one scan worker by default and support `-t 0` for automatic selection. Compilation uses a separate affinity/cgroup-aware half-CPU limit via `scripts/cargo`.
- Start with ordinary Cargo release; retain unwind for terminal cleanup. Optimizer variants require measurements before adoption.
- Use libc for audited descriptor operations, zstd with default features disabled for ncdu frames, and unicode-width for terminal column measurement. Primary API documentation was inspected at https://docs.rs/libc/, https://docs.rs/zstd/, https://docs.rs/unicode-width/ and Linux statx/getdents man pages. Cargo.lock pins actual resolutions; bundled C compression uses the same native job budget.
- The user's original workload, flags, cache state and RSS units remain unknown. Synthetic evidence cannot establish the reported performance objective.
