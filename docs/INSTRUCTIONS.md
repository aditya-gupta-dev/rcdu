# Instructions for the Rust rewrite agent

Read [GUIDE.md](GUIDE.md) before implementation. It analyzes all 17 Zig files and all 300 declared functions, the accounting semantics, interoperability, build settings, known limitations, and proposed Rust design. These instructions define how to execute the rewrite; they do not certify that the performance goal has already been achieved.

## Objective and scope

Implement a readable, maintainable, beginner-friendly **Linux-only Rust replacement** for the current ncdu 2.9.2 Zig project. Preserve the useful behavior of the CLI, exclusions, terminal browser, extended metadata, refresh, built-in/custom deletion, shell launch, raw/compressed JSON import/export, binary export, and low-memory indexed browsing. Keep explicitly documented behavior corrections from GUIDE.md in a decision log with focused tests. Do not turn this into a scan-only counter and call the full replacement complete.

The reported baseline is about 30 seconds and 77 MB RAM for 1,394,617 files and about 60 GB of accounted usage. Beat **both elapsed time and peak resident memory** on a comparable workload with correct results. First reproduce the baseline on the same machine/root/options. The exact baseline command, cache state, worker count, and memory units need to be recorded, not guessed. <=24 seconds and <=65 decimal MB are provisional stretch targets. Report relative results if the original dataset/hardware cannot be accessed; do not claim the user's goal is met from synthetic results alone.

Implement Linux now. All Linux filesystem/syscall/account-specific behavior belongs under `src/os/linux.rs` (split into a Linux directory only if its complexity warrants it). Shared interfaces live in `src/os/mod.rs`. Future Windows/macOS implementations belong in `windows.rs`/`mac.rs` when requested. Unsupported targets must fail clearly; do not create stubs that pretend to scan correctly. Portable serialization/UI/domain behavior should remain outside Linux files.

## Resource-limited compilation

Use **half the CPUs available to the process, rounded down, minimum one**, for Cargo jobs on every build, check, test, clippy, bench compilation, install, and native dependency build. Respect affinity/cgroup limits, not merely host CPU count. If available CPU quota is fractional, round the available count up before applying the half rule so a positive quota never becomes zero. If discovery fails, use one. A Rust available_parallelism-based helper is acceptable. Record the discovery method in the build wrapper.

On a Linux system with a recent `nproc` that honors the relevant process constraints, the basic shell wrapper is:

```sh
ncdu_available_cpus=$(nproc)
ncdu_build_jobs=$((ncdu_available_cpus / 2))
if [ "$ncdu_build_jobs" -lt 1 ]; then
    ncdu_build_jobs=1
fi
export CARGO_BUILD_JOBS="$ncdu_build_jobs"
export NUM_JOBS="$ncdu_build_jobs"
export CMAKE_BUILD_PARALLEL_LEVEL="$ncdu_build_jobs"
export RAYON_NUM_THREADS="$ncdu_build_jobs"

cargo build --locked --release -j "$ncdu_build_jobs"
cargo check --locked --all-targets -j "$ncdu_build_jobs"
cargo test --locked -j "$ncdu_build_jobs" -- --test-threads="$ncdu_build_jobs"
cargo clippy --locked --all-targets -j "$ncdu_build_jobs" -- -D warnings
cargo fmt --check
```

This example starts **after Cargo.lock exists**. Resolve dependencies once to create the lockfile, then use `--locked` in reproducible checks. Have `scripts/cargo` compute/export the limits dynamically and forward arguments so contributors do not need to remember them. Do not commit a machine-specific fixed `jobs = 6`; this inspected machine reported 12 online CPUs, but effective availability can differ. Check the installed nproc behavior and cgroup constraints before relying on it.

Cargo `-j` limits build jobs rather than strictly limiting every compiler/backend/native thread. Remove hard-coded `make -j8`; audit build scripts for parallelism overrides, use Make/CMake job settings or inherited jobserver correctly, avoid unrelated simultaneous builds, and use affinity/cgroup CPU controls if strict aggregate CPU occupancy is required. Do not overwrite system variables such as HOME or CODEX_HOME. Cap test execution separately as shown. The exported Rayon limit above is for build/test tools; do not accidentally inherit it as the scan-worker setting in performance runs—clear it or configure the scanner's pool explicitly when launching the application.

**Compilation jobs and scan workers are separate settings.** The user requires half-core compilation and multithreaded scanning; the scan pool should be tuned to the storage workload and exposed through `-t/--threads`. Preserve explicit `-t 1` and automatic `-t 0`; document any change in default worker selection. Avoid nested full-size scan/compression/Rayon pools.

## Implementation sequence with concrete gates

1. Preserve the Zig source, build recipe, man page and license/credits as reference. Store Rust entry points in `.rs` modules alongside it or a clearly documented subdirectory. Keep the old executable available under a distinct baseline path. No destructive source replacement or user-tree deletion is needed to begin.
2. Inventory observed behavior from GUIDE.md, source and fixtures. Add `docs/compatibility.md` recording each feature as pending/implemented/verified, and a decision log distinguishing preserved behavior, corrected defects and intentionally deferred work. Every feature needed for full parity must be finished before calling the rewrite complete.
3. Build a benchmark manifest and a correct single-worker Rust observation/model baseline. Establish raw-byte filenames, allocated/apparent sizes, device/inode scope, presence flags, errors, saturation and directory own-size accounting before tuning. Capture end-to-end time and RSS now so later changes have an internal baseline.
4. Add Linux descriptor-relative enumeration/metadata with a narrow safe interface. Start with reliable wrappers, then benchmark buffered getdents/statx alternatives. Preserve a fallback for unsupported statx and test it explicitly.
5. Implement dense arenas/name storage and a bounded fixed scan pool with parallel directory enumeration and local size accumulation. Add bottom-up ordinary totals and hardlink reductions. Verify termination/cancellation/FD ownership under adversarial schedules and uneven tree shapes before accepting speed gains.
6. Implement source/sink completion semantics, streaming single-worker JSON, staged multiworker JSON, block-based binary export, and indexed lazy reader. Cross-read Zig/Rust exports both ways. Preserve byte names and wire constants. Benchmark each mode separately.
7. Implement terminal browser, sorting/details/link navigation/help/config, refresh and shell behavior. Keep curses or the chosen terminal adapter on one owner thread. Test stdin/stdout redirection, /dev/tty, resize, lost TTY, interruption, panic/error cleanup and slow/fast UI intervals.
8. Implement deletion on disposable temporary trees only. Preserve no-follow traversal, confirmation, partial failure/abort, custom command environment and post-action refresh. Verify accounting after every mutation and never assume every stat failure means successful deletion.
9. Profile the full application against prior Rust and Zig. Make one material optimization at a time, retaining before/after results and rejecting unexplained degradations. Recheck peak RSS during aggregation, export, first browse and repeated refresh.
10. Finish contributor/build/run/benchmark instructions, feature checklist and evidence report. A second contributor should be able to add a sort field or exclusion fixture from documented examples without editing unsafe syscall code.

## Design rules

- Keep `main.rs` small; use a testable library/session API. Config parsing, traversal, accounting, storage, serialization, browser, terminal adapter and mutation policy have separate modules. Prefer one crate initially; introduce more crates only when a clear ownership/public API need warrants it.
- Use descriptive types/fields/functions; prefer explicit units such as AllocatedBlocks and ApparentBytes, and identity types EntryId/DirectoryId/InodeKey/BinaryRef. Avoid dense clever expressions that save lines while obscuring invariants.
- Code should explain ordinary behavior through names and structure. Do not fill files with comments restating syntax. Public contracts, contributor docs, nonobvious performance decisions and **unsafe SAFETY invariants** must still be documented. The user's self-documenting-code request does not justify unexplained unsafe code.
- Default to safe Rust. Restrict FFI/raw record parsing to audited boundaries; specify alignment, initialization, buffer validity, interior NUL, lifetimes, returned masks and descriptor ownership. Avoid Rust references to unaligned packed fields or unchecked enum conversion.
- Own descriptors with OwnedFd and borrow them deliberately. Reuse per-worker buffers. Ordinary file scanning must not read contents/open each file/build full absolute paths. Cache marker contents are the bounded exception.
- Preserve Linux filenames as bytes end to end. Convert/sanitize only for display. Imported malformed components must not enable filesystem actions through embedded NUL, slash, dot/dotdot or path traversal; define root-name handling separately from child basename validation.
- Keep entry records compact. Do not give every file a String, Vec, mutex, Arc, HashMap or complete path. Measure layout, side tables, name bytes, unused capacity and peak transient allocations; roughly 8 extra bytes/entry costs 11 MB at the reported count.
- Use a fixed worker pool and bounded task/output queues. Each directory has one enumerator; children may execute elsewhere. Workers blocked while publishing into a full queue must have a local/cooperative path that guarantees progress. Keep all task and child-completion lifetimes explicit.
- Publish child work only after incrementing outstanding accounting. Finalize a directory exactly once after enumeration and child completion. Empty queues alone do not prove termination. Cancel/join all workers and release descriptors on fatal setup/scan/output failures.
- Count ordinary sizes locally and merge coarse completion records. Parallel hardlink work must preserve totals in every affected ancestor; global first-seen dedup is incorrect. Shard identities only with a documented deterministic reduction for shared directory state.
- Progress snapshots/cancellation checks are bounded and responsive. UI never locks per-file model state for every redraw. Avoid one atomic/message/path copy per file unless measured evidence justifies it. Account for false sharing and aggregation latency.
- Refresh should retain prior data if root stat/open fails, then replace/reuse observations safely and recompute old/new identity contributions. Repeated refresh must not leak abandoned arena generations indefinitely. Stable/generational IDs prevent stale browser references.
- Maintain compatibility wire constants independent of in-memory layout. JSON needs byte-preserving legacy support; binary blocks need checked 24/28/40/56-bit packing, decompression limits, borrowed-cache lifetime discipline and cycle checks. Use crates when they match required behavior, not as an excuse to remove it.
- Any crates.io dependency is permitted. Check current primary docs and suitability, commit Cargo.lock, document meaningful choices, trim unused features and include native dependencies in build/performance evidence. Do not force a zero-dependency rewrite or assume a popular crate is the fastest choice.

## Compiler settings and experiments

Use optimized release artifacts for timing; debug results are not comparable to Zig ReleaseFast. Keep an optimized symbol-bearing profiling profile. Start with the GUIDE.md candidate release settings and compare ordinary Cargo release versus ThinLTO/codegen-unit variants before settling them. No optimizer setting replaces explicit checked/saturating accounting.

Retain terminal cleanup on panic and recoverable errors; unwind plus Drop is the initial policy. Consider panic=abort only with a verified terminal restoration strategy and measured benefit. Do not manually remove unwind sections from Rust artifacts as the old static recipe does. Distribution stripping is separate from scan heap memory optimization.

Keep default CPU target portable for primary tests. Label target-cpu=native, PGO, allocator changes, static musl, fat LTO and alternative compression implementations as separate experiments; give Zig equivalent treatment where applicable. Report exact flags and any functionality removed from an experimental build. Do not compare a Rust scan-only/stat-only executable to the full Zig browser model.

## Required correctness checks

Port behavior-focused cases from all existing Zig tests, then add meaningful integration/OS fixtures. Avoid tests that merely mirror method implementation. Include:

- Empty/ordinary/deep/wide trees; directory own sizes and descendant counts; allocated versus apparent usage, sparse files, saturating maximum conversion and counter capacity limits.
- Hardlinks within one directory, sibling subtrees, outside the root, across devices, unknown nlink, inconsistent metadata, repeated refresh and deletion; verify total/shared/unique at **every directory**, not only root.
- Symlinks to files/directories/dangling targets, -L, no-follow opens, same-device/cross-device link targets, replaced entry races and same-filesystem boundaries.
- Kernel-filesystem exclusion with selected kernel root still scanned, valid/invalid/short CACHEDIR.TAG, and descriptor cleanup under cache exclusions. Use isolated mount fixtures only where already available; clearly mark unavailable integration cases.
- Anchored filesystem-root and unanchored multi-component excludes, directory-only rules, duplicates, classes/escaping, wildcards not crossing slash, hidden display filters distinct from exclusions.
- Raw invalid UTF-8, controls, quotes, backslash, Unicode wide/combining display, large names/paths, interior-NUL malformed imports, and identity unaffected by display escaping.
- Config precedence, ignore-config, @ suppression, tilde paths, blank/long/final unterminated lines, -rr, -- separator, attached/equals arguments, legacy spelling and option ranges.
- JSON/compressed JSON compatibility, exact numeric boundaries, surrogate pairs, fractional mtime, field ordering, unknown fields/minor extensions, late error limitation and truncated/malformed streams.
- Binary cross-read/write, own versus cumulative fields, block-size boundaries, CBOR widths/negative deltas, invalid index/ref lengths, root validation, corrupted frames, cycle/resource limits and eight-block-cache eviction lifetimes.
- Pool completion while active workers discover children, bounded-queue saturation, skewed workloads, worker failure/spawn failure, cancellation, low descriptor limit and leak checks. Repeated scheduling stress must not hang or lose counts.
- Browser sort ties/natural ordering, parent row, saved selection, hidden toggles, hardlink jump path coherence, lazy capability restrictions, all help/key actions, TTY transitions and errors.
- Deletion confirmation defaults, abort/ignore/all, only scanned children, newly appeared children, replacement symlink directories, custom command success/failure, restat permission/I/O failures, and post-mutation hardlink recount. Run solely on owned temporary fixtures.

Permission fixtures must run without root privileges; otherwise inaccessible-directory assertions are meaningless. OS metadata changes during scans are nondeterministic; isolate fixtures or record expected observation uncertainty rather than hardcoding unstable block totals. Preserve copyright and MIT license notices; retain natural-sort upstream attribution. Run formatting, clippy, debug/release correctness tests and applicable man-page/license checks under the half-core build limit. Additional sanitizers/Miri/loom are useful for specific unsafe/concurrency concerns when supported; do not make unsupported tooling a fake passed check.

## Performance protocol and degradation checks

Create `scripts/benchmark` and `benchmarks/results/` with a versioned workload manifest and raw machine-readable results. Keep fixture generation and compilation outside measured execution. Each result records command, root, options, versions/build flags, hardware/storage/kernel/filesystem, CPU constraints, worker count, entry/directory/error/exclusion/link counts, cache state, wall/CPU time, peak RSS, exit status and checksum/normalized totals.

Use the current executable's `--ignore-config -0 --quit-after-scan` with identical root/options/thread count for scan benchmarks. The Rust implementation must support equivalent measurement. Include complete model construction and final hardlink/directory reduction; do not stop the timer at end of enumeration. Run separate interactive-mode and export benchmarks where appropriate. Measure first browsing frame and large-directory sort/lazy loading rather than hiding them from the full experience.

Use `/usr/bin/time -v` or a validated equivalent; GNU time reports maximum RSS in KiB, so record raw units and convert explicitly. Peak RSS must include all scan threads and native compression/allocator state. Parent RSS does not automatically include child process peaks; avoid helper processes in accepted scan measurements or report their resource use separately. Approximate progress bytes are not an accounting oracle.

Perform a warm-up and at least five alternating-order runs per implementation/worker setting. Report medians and spread plus raw observations, and repeat when results overlap noise. Keep warm-cache versus isolated cold/first-run results separate. Never run cache-drop/root/remount commands or scan/deletion experiments that modify user data to obtain a cleaner benchmark. If cold-cache controls are unavailable, state it.

Test workers 1, 2, 4, half available CPUs and all available CPUs (deduplicate/cap to meaningful values); compare both equal-worker performance and best verified settings. Report where increased parallelism slows HDD/network scans or inflates memory. Build limits do not prescribe runtime worker limits. Maintain adversarial broad/deep/wide/link-heavy fixtures in addition to the representative user tree.

For each hot-path/layout/dependency/compiler change, compare against the preceding Rust artifact and Zig on the same manifest. Investigate **>5% median elapsed-time or >5% peak-RSS degradation** as a provisional threshold. Repeat noisy results, identify the cost, and fix or document a justified tradeoff; never silently accept an unexplained regression because one workload got faster. Correctness failures are blockers even if speed improves. Record time/RSS together so a speed win cannot conceal a memory regression.

Use perf/strace/allocator profiling only in separate diagnostic runs; their overhead invalidates direct baseline timing. Attribute cost among metadata calls, enumeration, allocation, scheduling, hardlink reduction, output compression, UI polling and sorting. Record unavailable tools/checks honestly. Keep completed measurements; do not cherry-pick best runs or substitute average throughput for end-to-end latency.

## Completion and handoff

The rewrite is complete only when the compatibility checklist is verified or an explicit user-approved scope reduction exists, required tests/checks pass, Linux boundaries remain isolated, the code/contributor docs are understandable, and equivalent measurements demonstrate both time and memory improvement. If the user's dataset cannot be measured, say that the performance objective remains unverified even if the implementation is finished.

Provide exact build/run/benchmark commands, feature/behavior corrections, dependency/compiler decisions and evidence, before/after medians/RSS/spread, remaining platform/fixture limitations, and raw result locations. Never claim a particular Rust design or crate beats Zig without those results. Maintain GUIDE.md and this file when implementation evidence changes a proposed decision.
