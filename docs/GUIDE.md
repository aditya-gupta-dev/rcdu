# Porting ncdu 2.9.2 from Zig to Rust

This guide analyzes the checked-in implementation, not a hypothetical disk scanner. Read it together with [INSTRUCTIONS.md](INSTRUCTIONS.md), which gives the rewrite agent its execution requirements. The scope is a Linux Rust replacement with the existing CLI, terminal browser, exclusion rules, refresh/deletion behavior, JSON interoperability, and binary export/lazy browsing. The reference analysis below is preserved; the implemented Rust project is described in [architecture.md](architecture.md), [compatibility.md](compatibility.md), [decisions.md](decisions.md) and [measurement.md](measurement.md).

## Evidence and performance objective

The user reports approximately **30 seconds and 77 MB RAM for 1,394,617 files covering about 60 GB**. This is a reported baseline, not a measurement reproduced here. Its command, thread count, filesystem, storage medium, cache state, whether directories are included in the count, and meaning of MB/RAM are unknown. Do not silently invent them. The ratio is about 46,487 files/second and 55.2 decimal bytes/file including process overhead; that memory ratio is only illustrative until file/directory counts and RSS units are established. The 60 GB describes capacity accounted for, not bytes read: this program reads directory entries and metadata, not file contents, apart from cache marker files and imports.

Success requires correct results **and both faster elapsed time and lower peak RSS** on an equivalent workload. Establish a same-machine Zig baseline before claiming success. Provisional stretch target: <=24 seconds and <=65 decimal MB; these are proposed engineering targets, not promises. Also compare identical worker counts and compare each implementation's best tested worker setting. A multithreaded Rust run beating Zig's default single-threaded run alone does not establish a language or implementation improvement.

There are 16 Zig files under `src/` plus `build.zig`, totaling 6,831 lines including the build file. The function ledger below accounts for every declared function, including nested functions, inline encoding helpers, duplicate names, and both conditional LineReader implementations. Source line numbers refer to this snapshot. Reasons explicitly present in source comments are identified where relevant; other explanations are engineering inferences from control flow and layout, not claims about the author's unrecorded intent. `README.md`, `Makefile`, `ChangeLog`, and `ncdu.1` provide additional context. At the original inspection Zig was unavailable and the workspace had no commits. Implementation subsequently preserved this source under `reference/ncdu-2.9.2`, built it with verified Zig 0.15.2 and ran its tests. Current Git history and measured evidence supersede those initial environment observations. No local AGENTS.md was found.

## Architecture and why the boundaries exist

```text
CLI/config -> scan or JSON import or binary import or memory replay
                    |
             sink lifecycle + progress
                    |
         memory tree | JSON writer | binary block writer
                    |
          terminal browser / lazy binary reader
                    |
              refresh / delete / shell
```

Sources produce observations; sinks decide their representation. This prevents separate scanners for the browser and each export format. `sink.Dir` completion waits for all child tasks, not just the end of parent enumeration, because cumulative sizes and child references must be complete before a directory is finalized. Browser and terminal utilities are separate because sorting/navigation and drawing have different state and correctness concerns. The current globals simplify one-process/one-session operation but hide ownership, make testing harder, and couple model references to a mode flag. Rust should make configuration immutable during scans, use explicit session ownership, and separate browser state from scan state.

The Zig scanner already implements a pool. Default `threads = 1`; `-t 0` selects detected CPU count. A shared 16-slot LIFO queue gives out directories; a worker scans locally depth-first if it cannot enqueue a child. A mutex/condition variable plus the number of waiting workers determines completion. Additional threads have 128 KiB stacks. All workers use descriptor-relative traversal; thread zero also drives terminal events. Thus ordinary Rust recursion, unlimited spawning, or a path-based walk with per-file allocation can lose to the existing code.

## Semantic contract before optimization

### Sizes, records, and overflow

Keep apparent size (`st_size`) separate from allocated size (`st_blocks * 512`). Directories themselves have both sizes. Sparse files demonstrate why reading only file lengths is wrong. `Blocks` is u60 to share a 64-bit word with a signed three-bit entry kind and the extended-data flag. Arithmetic commonly saturates; Rust release arithmetic wrapping is not an equivalent behavior. Use named conversion helpers and explicit checked/saturating operations. Negative OS size/block/time inputs are clamped by the scanner; inode/device identifiers are truncated only to their destination widths. Avoid unintended casts.

The current byte-aligned records have a 24-byte Entry prefix on conventional 64-bit targets: flags/blocks, size, next reference. File is that prefix plus a NUL-terminated inline name; Dir adds child/parent references, shared totals, item count, and device/error flags (64 bytes before name on such targets); Link adds parent, two ring pointers, inode, and flags (60 bytes before name). Optional Ext occupies 19 bytes immediately *before* Entry. These are inferred layouts, not measured ABI results. An enum containing the largest variant, `String`, `Vec` per entry, `Arc` per file, or full paths per file will likely exceed the memory target. Stored full paths also multiply shared prefixes. Preserve names as bytes and reconstruct paths only for UI/errors/actions.

Do not mirror these layouts using unsafe Rust packed references. Use dense typed arenas, compact IDs, a byte arena for names, and side tables for directory, hardlink, and optional extended fields. A 32-bit ID requires explicit capacity checking; use segmented arenas or wider IDs when limits are exceeded. Quantify `size_of`, capacity slack, allocation headers, hash tables, name bytes, progress buffers, worker stacks, live descriptors, and transient merge memory. Roughly 8 extra bytes per entry costs 11.2 decimal MB at the reported count. Measure peak memory during finalization and browsing, not just final model capacity.

### Hardlinks and what deleting a directory can reclaim

The identity is `(device, inode)`, never inode alone. A normal file is counted directly. Hardlinked observations (`nlink > 1`, after directory classification) are retained individually for listings, but cumulative totals count the inode once in **each ancestor containing an observation**. Shared totals mark an inode when an ancestor contains fewer links than the effective total link count, including links outside the scan. Unique/reclaimable estimate is saturating `total - shared`. Example: links in sibling directories A and B contribute once to A, once to B, and once to their parent; A and B show shared space. If a third link exists outside the root, the root also shows shared space. A global first-seen dedup set gives wrong sibling totals.

Memory mode maintains a circular ring per identity and delays/reverses hardlink statistics before mutations. `inodes.setStats` collects all affected ancestors and their observed link counts. If observations disagree on nlink, it falls back to observed count; sizes are taken from one representative without consistency checking. Unknown nlink from older JSON is zero. Binary export uses per-directory inode maps and bottom-up merging instead; fully contained groups can be dropped after transferring their contribution. These two paths have different unknown-nlink behavior. Preserve and test their existing compatibility boundaries, or document an intentional correction rather than assuming equivalence.

Refresh/deletion must remove old hardlink contributions **before** changing rings, sizes, or counts, then recompute affected identities. A changed link count outside the refreshed subtree can leave estimates stale; the man page acknowledges the need for a full refresh. Hardlinks are not reflinks or shared compressed extents: this model does not estimate those physical sharing mechanisms.

### Traversal, symlinks, exclusions, and errors

`fstatat` with SYMLINK_NOFOLLOW observes entries relative to an open parent. Directories are opened with no-follow. The root is statted with following enabled and may be resolved to an absolute path. `-L` follows file symlinks only; a directory target is never traversed, failed target stat keeps the original symlink observation, and a hardlink target on a different device is treated as ordinary to avoid inherited-device dedup errors. Same-device targets with nlink >1 still use hardlink logic in code. The manual's claim that each followed file symlink is unique is broader than the implementation; use differential fixtures to settle the promised behavior.

`same_fs` compares the child device to its current parent device, rooted initially at the scanned device. Cache exclusion opens CACHEDIR.TAG and compares exactly the first signature bytes `Signature: 8a477f597d28d172789f06886806bc55`; existence alone is insufficient. Kernel-filesystem exclusion runs only on opened child directories crossing a device boundary, never the explicitly selected root. `fstatfs` errors currently fail open. Preserve visible exclusion reason entries with zero usage; excluding/hiding an item does not mean the same thing. Hidden filters are browser-only and include dot-prefix, tilde-suffix, and excluded entries.

Patterns are component-oriented libc fnmatch matches. Leading `/` anchors to the filesystem root, not the chosen scan root. Unanchored multi-component patterns may start at any level; a trailing slash restricts to directories. Wildcards do not cross `/`. Match returns three states: no match, exclude any kind before stat, or exclude only directories after stat. Literal hashing avoids fnmatch in the common case. Do not substitute gitignore semantics or an arbitrary glob crate without compatibility tests.

Missing/stat-failed items become error records; unreadable directories retain their own metadata and a read-error bit. Ancestors aggregate descendant-error bits. Files can disappear/change during enumeration, so results are observations rather than an atomic snapshot. Keep partial results and diagnostics. Preserve raw Linux path bytes; never use lossy UTF-8 for lookup, identity, export, or deletion. Sanitize only the display: current UI renders invalid/control bytes as `\\xHH`.

### CLI and session behavior

System config `/etc/ncdu.conf` precedes user config `$XDG_CONFIG_HOME/ncdu/config` or `$HOME/.config/ncdu/config`, then CLI overrides scalars. Repeated excludes accumulate. `--ignore-config` is detected before loading any config. Config lines support whitespace or `=` between option and value, comments, and `@` to ignore option errors; values are not a shell grammar. Tilde expansion is specific to config exclusion paths/patterns. Retain `--`, short clusters, attached values, long `--key=value`, `-r`, `-rr`, and legacy `eigth-block` spelling. Only one scan directory/import/export selection is accepted. `--quit-after-scan` is a real undocumented benchmarking interface.

Imported trees default to disabled shell/deletion/refresh unless explicitly enabled. Lazy binary browsing additionally refuses deletion and refresh even if options request them. Stdout can carry export bytes while curses uses `/dev/tty`; progress belongs on stderr. UI modes are none/line/full; drawing is throttled to 100 ms or 2 seconds. Counts shown during scanning are approximate: hardlink bytes are divided by nlink for progress; authoritative totals arrive at finalization. Do not compare progress bytes to final accounting.

### Serialized formats

JSON header is `[1,2,{metadata},root]`; directories are arrays beginning with their own object, ordinary entries are objects. Device inherits from the parent unless emitted; nlink/ino/hlnkc describe links; optional presence bits distinguish missing metadata from zero. Files with invalid UTF-8 are emitted as raw string bytes, so some legacy exports are not strict UTF-8 JSON. A strict generic JSON parser is therefore insufficient by itself. The parser accepts surrogate pairs and fractional mtime (discarding fractions), ignores unknown fields/minor versions/trailing elements, and checks major version. Build a byte-preserving compatibility layer if a crate cannot handle this contract; use streaming, not a whole-document DOM.

JSON writing is depth-first and single-threaded. Multithreaded scanning falls back to memory with statistics disabled, then replays the tree to JSON. This is deliberate: nesting cannot be arbitrarily interleaved. The open directory-object mechanism can record an enumeration error only before its first child is written, a format/writer limitation. Do not accidentally claim the JSON path has binary export's memory behavior.

Binary signature is the eight bytes `BF 6E 63 64 75 45 58 31` (`\\xbfncduEX1`). Data blocks have a big-endian four-byte header, four-byte block number, compressed payload, and matching four-byte footer. Headers pack a four-bit kind and 28-bit total length. The final index block has header, eight-byte index entries, eight-byte root reference, footer. An index entry packs a 40-bit file offset and 24-bit compressed block length. Item references pack a 32-bit block number and 24-bit uncompressed offset; same-block backwards references use CBOR negative deltas. Items never span blocks. CBOR maps use numeric keys, byte-string names, signed EType numbers, indefinite map endings, and optional fields. Directory records are emitted after descendants so cumulative statistics are known. Binary export always uses Zstandard, regardless of the JSON compression toggle.

Stable ItemKey values: type=0, name=1, prev=2, asize=3, dsize=4, dev=5, rderr=6, cumasize=7, cumdsize=8, shrasize=9, shrdsize=10, items=11, sub=12, ino=13, nlink=14, uid=15, gid=16, mode=17, mtime=18. Stable EType values: dir=0, reg=1, nonreg=2, link=3, err=-1, pattern=-2, otherfs=-3, kernfs=-4. Treat these as wire constants, independent of Rust memory layout.

Binary reader seeks to the final index and lazily resolves requested listings, with eight decompressed LRU blocks. It copies records/names into an arena before cache eviction. Lazy browsing has low startup/memory cost; eagerly importing everything removes that advantage. Despite comments proposing a streaming binary reader, only random-access reading and DFS replay are implemented; non-seekable binary stdin must not be advertised as supported without implementing it. Validate index lengths, compressed lengths, offset arithmetic, decompression bounds, field types, backward references, sibling/child cycles, embedded NULs, and nesting/resource limits. Existing permissive parsing must not justify memory corruption or infinite loops.

## Build configuration: every setting and its reason

### build.zig

| Setting | Why it exists | Rust decision |
| --- | --- | --- |
| `standardTargetOptions` | Native builds and user-selected cross targets share one build description. | Explicit Linux target support first; keep CPU-specific benchmarking separate from distributable builds. |
| `standardOptimizeOption` | Select debug, safe release, fast release, or size-oriented optimization. | Dev checks for diagnosis; benchmark optimized release. Do not equate Rust wrapping arithmetic to Zig saturating operators. |
| optional `-Dpie` / `exe.pie` / test PIE | Caller override of target-dependent position-independent executable defaults. | Keep target defaults unless packaging requires otherwise; inspect ELF output instead of assuming a flag improves scan speed. |
| `-Dstrip` default false / module strip | Remove debug information for smaller distributed artifacts, while default builds stay debuggable. | Strip a distribution artifact; retain an optimized profiling artifact with symbols. Stripping is not a heap/RSS algorithm optimization. |
| root `src/main.zig`, target, optimize | One module supplies executable and test imports, so both use consistent dependencies/settings. | Tiny main plus testable library modules. |
| `link_libc=true` | C allocation, OS/stat ABI, locale/account lookup, and C libraries require libc. | Prefer safe OS wrappers; libc stays available for narrowly scoped ABI needs. |
| `ncursesw` system library | Wide-character terminal UI, key handling, terminfo, color. | Preserve with bindings initially, or replace behind a terminal adapter after PTY parity tests. |
| `zstd` system library | Compressed JSON streams and compressed binary blocks. | Select crate/system linking deliberately; report linked library version in comparisons. |
| Darwin `headerpad_max_install_names` | Source says useful to package maintainers; reserves linker header room for install-name changes. | No Linux analogue needed. Keep future mac-specific settings out of Linux modules. |
| executable/install/run/test steps | Install artifact, run it with forwarded args after install, and execute tests from same root module. | Cargo build/run/test plus explicit install/package docs; no need to preserve Zig build mechanics. |

### Makefile and static dependency recipe

`ZIG ?= zig` allows toolchain selection; PREFIX/BINDIR/MANDIR allow packaging layouts. `ZIG_FLAGS ?= --release=fast -Dstrip` makes `make`/release optimize for runtime and remove debug information. `debug` leaves compiler defaults. `NCDU_VERSION` extracts the source version for archive names. `clean` removes build outputs; install uses executable mode 0755 and man-page mode 0644. Distribution packaging selects tracked files, dereferences temporary symlinks, and sorts archive entries for stable ordering. Preserve useful packaging intent, not shell implementation quirks. Test target runs Zig tests, mandoc lint, and REUSE license lint.

Static builds compile vendored C dependency sources separately, because the author explicitly chose not to write Zig build descriptions for them. The commented `pkg-config` approach reportedly produced dynamic binaries; commented `-Drelease-fast=true` belongs to an older recipe, not the active build. Do not copy obsolete flags.

| Static flag or group | Reason and cost |
| --- | --- |
| `make -j8` (zstd and ncurses) | Parallel C compilation; a fixed machine-independent choice. Replace with the user's half-core budget for **all** build tools. |
| `ZSTD_LIB_DICTBUILDER=0` | Avoid unused dictionary-training machinery. Normal compression/decompression remains needed. |
| `ZSTD_LIB_MINIFY=1` | Select library size reductions; exact effects depend on vendored zstd version, not provided here. Audit that version before reproducing. |
| `ZSTD_LIB_EXCLUDE_COMPRESSORS_DFAST_AND_UP=1` | Cut higher compressor implementations from small static artifacts. Verify accepted high compression levels against the resulting dependency; do not assume all levels behave identically. |
| `CC`, `LD = zig cc --target=$*`; `AR=zig ar`; `RANLIB=zig ranlib` | Same target-aware toolchain for dependency compilation, linking, and archives; necessary for cross compilation. |
| ncurses `--prefix` / `--host` | Stage headers/libraries locally and select cross target. |
| `--without-cxx --without-cxx-binding --without-ada` | This app uses the C ncurses interface; language bindings add no value. |
| `--without-manpages --without-progs --without-tests` | Build library distribution only, omit ancillary tools/docs/test executables. |
| `--disable-pc-files --without-pkg-config` | Recipe passes library/include paths directly, so package discovery files are unnecessary. |
| `--without-shared --without-debug` | Static release library rather than shared/debug variants. |
| `--without-gpm --without-sysmouse` | No mouse integration is used by this keyboard-driven UI. |
| `--enable-widec` | Unicode-aware curses symbols/UI. |
| terminfo default/search dirs | Find installed terminal definitions at standard system locations. |
| `--with-fallbacks="screen linux vt100 xterm xterm-256color"` | Useful terminal capability fallbacks when no external terminfo is available. |
| `CPPFLAGS=-D_GNU_SOURCE` | Enable GNU libc extensions during C dependency compilation. Distinct from c.zig's X/Open feature definition. |
| `build-exe -target $* -Inc/include -Izstd` | Cross target and staged C headers. |
| `-lc nc/lib/libncursesw.a zstd/libzstd.a` | Link libc and the explicitly static dependency archives. |
| `--cache-dir zig-cache` | Isolate intermediate compiler outputs inside the static staging directory. |
| `-static -fstrip -O ReleaseFast` | Self-contained fast artifact with reduced debug data. Static libc target is musl in named Linux archives. |
| `strip -R .eh_frame -R .eh_frame_hdr ... \|\| true` | Remove unwind tables for smaller archive; tolerates host strip failing on foreign architecture (explicit source comment). Rust unwinding/backtraces/cleanup make blindly copying this inappropriate. |
| x86_64, x86, aarch64, arm musl/musleabi targets | Existing Linux static distribution architectures; support each only when tested, not by claiming Linux means x86_64 only. |

`c.zig` defines `_XOPEN_SOURCE=1` specifically to expose wcwidth. `@branchHint(.cold/.unlikely)` marks panic, OOM, refill/flush/slow parser paths for code layout; `inline fn` encourages tiny wire conversion helpers; comptime specialization avoids runtime generic dispatch. Rust should start with ordinary inlinable helpers and move cold diagnostics out of hot loops. Only retain forced inline/cold attributes when profiling supports them.

### Proposed Rust profiles

Start from ordinary optimized Cargo release, then measure ThinLTO and codegen-unit variants. A candidate configuration is:

```toml
[profile.release]
opt-level = 3
lto = "thin"
codegen-units = 1
strip = "debuginfo"
panic = "unwind"

[profile.profiling]
inherits = "release"
debug = "line-tables-only"
strip = "none"
```

Optimization level, LTO, codegen units, symbols, and panic strategy are independent tradeoffs; none guarantees a faster filesystem scan. Keep unwinding for terminal guards until an alternative cleanup strategy is verified. Use explicit saturating totals regardless of profile defaults. These options follow the [Cargo profile reference](https://doc.rust-lang.org/cargo/reference/profiles.html); recheck the pinned toolchain before adoption. `target-cpu=native`, fat LTO, PGO, allocator swaps, or abort-on-panic are experiments with separate results, never hidden advantages in the primary comparison.

## Rust architecture and performance decisions

Suggested layout (adapt as evidence warrants):

```text
src/main.rs                 process entry and exit status
src/lib.rs                  reusable session API
src/config.rs               validated config and precedence
src/cli.rs                  option/config parsing boundary
src/model/{mod,arena,totals,hardlinks}.rs
src/scan/{mod,scheduler,worker,progress}.rs
src/os/{mod,linux}.rs        OS metadata, enumeration, descriptor operations
src/sink/{mod,memory}.rs     ownership and completion lifecycle
src/format/{mod,json_reader,json_writer,binary_reader,binary_writer}.rs
src/browser/{mod,sort,navigation,details}.rs
src/ui/{mod,terminal,format,display}.rs
src/delete.rs               explicit deletion state machine
scripts/{cargo,benchmark}   reproducible resource-limited tooling
benchmarks/                 fixtures, manifests, results
```

Implement only Linux. Select it in `os/mod.rs` with cfg and give other targets a clear unsupported error. Future implementations belong in `windows.rs` and `mac.rs` when work is requested; do not scatter cfg checks or create fake implementations. Domain code consumes an OS-neutral observation with raw name bytes, file kind, device/inode/link count, sizes, and optional extended fields. Handles use OwnedFd/BorrowedFd ownership. Trait objects per file are unnecessary; choose static dispatch at module or scan setup boundaries. A single small crate is easier for beginners than premature multi-crate infrastructure.

### Linux metadata and enumeration

First implement a correct descriptor-relative baseline, then compare reusable buffered enumeration with `getdents64`/safe wrapper access. `d_type` can be DT_UNKNOWN and never supplies allocated block counts. Directory hints are useful for prefilters, not a replacement for metadata. Skip `.` and `..`; validate record lengths and NUL termination if parsing raw records. These constraints come from [Linux getdents documentation](https://man7.org/linux/man-pages/man2/getdents.2.html).

Measure `fstatat` against `statx` requesting required fields. Check returned masks and provide a correct fallback if statx is unavailable or blocked; do not suppress unrelated errors. Default synchronized metadata semantics are the starting point; AT_STATX_DONT_SYNC can change freshness on remote filesystems. Basic allocation/inode/link data remains necessary in non-extended mode. See [Linux statx documentation](https://man7.org/linux/man-pages/man2/statx.2.html). Neither smaller masks nor raw syscalls guarantee speed. Prefer [rustix filesystem wrappers](https://docs.rs/rustix/latest/rustix/fs/index.html) where the pinned crate exposes the needed operations; use libc only for an unmet boundary with tested layouts.

Do not add file-content reads, canonicalization for every entry, a pre-count pass, unbounded io_uring requests, or opening every ordinary file. Reuse worker name/entry/metadata buffers. Budget descriptors by queue size, local DFS depth, and worker count; bounded queues alone do not bound deep traversal descriptors. For very deep trees, retain logical traversal state and reopen via safe descriptor chains under a deliberate budget. Account for the performance cost and race policy. Keep root/mount handling explicit; blanket visited-directory suppression would change bind-mount/path accounting.

### Scheduling, accumulation, and memory

Use a fixed pool with an explicit scan-worker option; worker count is independent of build jobs. Start conservative for HDDs and benchmark 1/2/4/half/all available CPUs on actual storage. Each directory has one enumerator; children can be stolen as coarse tasks. Never spawn one thread or create one Arc/message per file. Keep worker-local byte/name arenas and counters, then combine directory batches. A bounded overflow path must execute work locally or cooperatively rather than deadlocking workers that all block while producing tasks.

Termination needs an outstanding-work invariant: increment before publishing a child, decrement after that task and its required completion work are committed, and wake all waiters on cancellation or final zero. An empty queue does not mean done while another worker can discover children. Use owned task/descriptor lifetimes and exactly-once completion. Directory aggregation runs bottom-up; ordinary totals can be computed locally in parallel and merged once per child. Hardlink identities can be sharded by `(dev,ino)` after enumeration, but ancestor totals need a second controlled reduction. Avoid simultaneous mutation of the same directory from multiple shards without ownership/locking. Test skewed million-entry single-directory and deep-chain trees, which expose scheduling limits that a broad balanced tree hides.

Progress snapshots should amortize counters (e.g. every 256–4096 entries, measured) and be cache-separated when false sharing appears. Keep cancellation checks responsive even during long single-directory scans and final hardlink reductions. UI stays on its owner thread; workers never call terminal functions. Output backpressure and compressed block buffers are bounded; compression threads must share a deliberate total thread budget with scan workers.

Worker arenas currently intentionally outlive scans and avoid per-entry free costs, but repeated refreshes retain abandoned records. Rust should provide explicit arena lifetime, reclaimed generations, and stable/generational IDs so refresh/deletion cannot leave dangling references. Compact IDs plus reclaimable name ranges need a documented policy; never compact the arena while the browser holds indices that will change. Optimize initial scan without sacrificing bounded repeated-refresh growth.

### Crates and readability

Any crates.io crate is allowed. Choose for measured value and maintainability, not a dependency-count contest. Candidate roles are rustix/libc for OS calls, crossbeam-deque or a small explicit scheduler for tasks, zstd for compression, clap for CLI if its grammar matches, and a curses adapter or ratatui/crossterm for TUI after compatibility work. These are candidates, not locked versions or claims of current compatibility. Inspect current primary crate docs, pin resolved versions in Cargo.lock, keep unused features off, and measure scan RSS/startup/native linking effects. A general walker may provide a valuable correctness prototype but must be measured against descriptor-relative scanning. Generic serde_json cannot simply replace byte-preserving legacy import/export.

Use descriptive types such as AllocatedBlocks, ApparentBytes, EntryId, DirectoryId, InodeKey, ScanOptions, and DirectoryCompletion. Expose small methods that state ownership and behavior. Error variants should distinguish unsupported syscall, inaccessible entry, cancelled scan, malformed import, and fatal root failure. Avoid comments narrating obvious syntax. Public API contracts, contributor documentation, and concise unsafe SAFETY explanations remain necessary where names cannot express an invariant. A beginner should be able to add a sort column without learning raw syscalls and add an OS backend without editing hardlink accounting.

## Existing issues to fix deliberately

1. `scan.Thread.run` handles an iterator failure using the outer queued `dir.sink` rather than the currently scanned `d.sink`. Attribute it to the current directory and add a nested failure fixture.
2. `deleteCmd` treats any restat error as deletion; only an established missing-path outcome should remove the model observation. Access/I/O errors remain uncertain and visible.
3. `json_import.uintTail` wrapping arithmetic plus `newv < v` is not a complete overflow detector. Rust must check multiply/add; test values that wrap to a value still above the prior value.
4. Recursive model reset, replay, import, skipping, and deletion can overflow on hostile/deep trees. Prefer explicit stacks and tested resource bounds.
5. `mem_sink.getEntry` may clear the `isext` discriminator when reusing an extended allocation for non-extended data. Optional data was allocated before the entry, and destroy derives allocation start from the discriminator. Current arenas obscure the consequence. Rust storage presence and display preference must be separate.
6. Source readiness masks `(counter & 65) == 0` are **not** a 64-entry modulo timer (65 is hexadecimal 0x41). Replace with explicit batched/time throttling and compare cancellation latency.
7. Device/inode/stat inconsistency, saturation followed by subtraction, item-count limits, mmap/borrow lifetimes if introduced, and unbounded import recursion need explicit policies; no silent overflow or dangling references.
8. `info.keyInput` jumps to a hardlink parent using `dir_parent` without rebuilding `dir_path` and the parent reference stack. Verify navigation coherency with a sibling-link jump and fix in Rust.
9. `ncdu.1` describes signed 64-bit clipping although current sizes are u64 and blocks u60; it lists compression levels 1–19 while argConfig accepts 1–20. Source behavior and documentation disagree. Record the chosen public compatibility behavior and update docs; never guess that prose outranks observed code in all cases.
10. Binary size bounds differ: headers allow 28-bit lengths, but item offsets and index compressed-length fields are 24-bit, and reader rejects decompressed lengths >=2^24. Enforce the tightest representable limit before encoding; do not widen one field independently.

These findings are static-review observations; they have not been executed here. The rewrite must use focused fixtures to confirm behavior and document deliberate corrections.

## Performance and correctness validation

Record the exact Zig command/build and Rust command/build, tools/dependency versions, hardware, storage type, kernel/filesystem/mount options, CPU affinity/cgroup limits, options, error/exclusion counts, total entries/directories/hardlink identities, and cache regime. Normalize GNU time's KiB RSS explicitly when discussing the reported MB. Benchmark complete scanning **through directory and hardlink finalization**, then separately browser startup, large-directory sort, refresh, JSON/compressed JSON, binary export/lazy browse/import. A scan-only accumulator that discards the browsing tree is not comparable to the 77 MB interactive model.

Use `--ignore-config -0 --quit-after-scan` for both programs when measuring noninteractive scan throughput, with the same root/options and explicit worker setting; separately reproduce the user's original interactive/UI mode. Capture `/usr/bin/time -v` wall time, peak RSS, CPU, I/O and exit code. Benchmark manifests may need an external GNU time package. Separate warm-cache repeated runs from first-run/controlled cold-cache results. Do not drop global caches, remount devices, run as root, or delete user files as part of measurement. If isolated cold-cache control is unavailable, label the limitation.

Alternate run order across implementations; use at least five measured warm-cache runs after a warm-up, report median plus spread and raw observations. Use more repetitions when the gap is within noise. Compare broad, deep, wide, sparse, empty, hardlink-heavy, invalid-name, excluded, extended-mode, inaccessible and disappearing-file workloads. Run permission tests as an unprivileged user. SSD/NVMe/HDD/network results belong in separate reports. Thread scaling can increase I/O contention and peak memory; retain slower results rather than cherry-picking.

Per performance-sensitive change, compare the preceding Rust artifact and Zig using the same fixture manifest. A >5% median time or >5% RSS increase is a provisional investigation threshold, not a statistical proof. Repeat if variance overlaps; explain tradeoffs and avoid accepting unexplained regressions. Gate final success on materially lower time and RSS, not on microbenchmarks alone. Profile syscall count/latency, directory queue contention, allocator costs, hardlink finalization, cache misses and UI polling as tools permit. `strace`/perf perturb timing; use diagnostic runs separately from accepted timing runs.

Correctness comparison uses normalized raw-byte paths, kind, own sizes, per-directory cumulative totals, shared/unique totals, descendant counts, presence of extended metadata, error flags, and exclusions. JSON/binary record order, timestamps, compression bytes, and natural sort representation may differ without semantic differences. Raw-byte filenames require a byte-aware normalizer, not strict JSON parsing. Keep root count semantics separate from progress count: sink.createRoot does not increment files_seen. Cross-read both directions: Zig reads Rust exports and Rust reads Zig exports, including legacy fixtures. Use complete browser/deletion PTY tests on disposable temporary fixtures only.

## File and function ledger

Each entry below identifies purpose, the reason for the implementation choice, and the porting consequence. The surrounding semantic/build sections supply shared invariants rather than repeating them hundreds of times. Source references are relative links so the document remains usable after checkout elsewhere.

### build.zig

The only Zig file outside src. Its build function defines dependency, target, installation, run, and test policy; each flag is analyzed above.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [build](../reference/ncdu-2.9.2/build.zig#L6) (6) | Creates a shared main module, links libc/ncursesw/zstd, applies target/optimization/strip/PIE choices, adds Darwin header padding, installs ncdu, forwards run arguments, and runs unit tests. Sharing the module keeps dependencies consistent; Rust uses Cargo profiles plus separate packaging checks. |

### src/main.zig

Process orchestration owns version 2.9.2, the allocator, global config, application state, standard streams, and event timer. Defaults are part of compatibility: one worker, compression level four, no extended/follow/cache/kernfs exclusions, descending allocated-size sort, natural ordering, visible hidden items, shared-size column, and deletion confirmation. Optional can_* values distinguish explicit overrides from imported-tree defaults.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [wrapAlloc](../reference/ncdu-2.9.2/src/main.zig#L44) (44) | Retries libc allocation through ui.oom so callers treat allocation as infallible. This explains catch unreachable throughout the project. Rust should use explicit fallible reserve at major boundaries and restore the terminal on failure rather than promise all standard allocations are recoverable. |
| [panic.panicFn](../reference/ncdu-2.9.2/src/main.zig#L67) (67) | Marks failure cold and resets curses before the default panic prints diagnostics. Without cleanup the terminal can remain unusable. Use a panic hook plus terminal guard; account for abort versus unwind behavior. |
| [Args.Option.is](../reference/ncdu-2.9.2/src/main.zig#L139) (139) | Requires the token to be an option before comparing bytes, preventing a positional filename from being interpreted as a flag. Retain this distinction even if a CLI crate supplies tokenization. |
| [Args.init](../reference/ncdu-2.9.2/src/main.zig#L144) (144) | Initializes parser state over the provided argument slice without copying every token. Borrow OsStr arguments in Rust and keep config token ownership explicit. |
| [Args.pop](../reference/ncdu-2.9.2/src/main.zig#L148) (148) | Consumes the front token while returning its borrowed bytes. Centralizes advancement so option/value parsing cannot disagree on position; use an index or iterator with explicit lifetime. |
| [Args.shortopt](../reference/ncdu-2.9.2/src/main.zig#L154) (154) | Returns one short option and retains the remainder of its cluster in a tiny reusable buffer. This supports both -rr and attached values without allocation; test equivalent crate behavior. |
| [Args.die](../reference/ncdu-2.9.2/src/main.zig#L162) (162) | Either produces InvalidArg for @ config suppression or exits with a diagnostic. The distinction enables optional config entries; Rust returns a typed parse error and lets the process boundary decide to exit. |
| [Args.next](../reference/ncdu-2.9.2/src/main.zig#L170) (170) | Handles --, positionals, short clusters, and long equals values while detecting unexpected arguments and lone dash. Preserve exact grammar through fixtures rather than blindly adopting clap defaults. |
| [Args.arg](../reference/ncdu-2.9.2/src/main.zig#L194) (194) | Consumes attached short remainder, long equals value, or next token in priority order. Missing values name the offending option. Keep parser consumption and error context testable. |
| [argConfig](../reference/ncdu-2.9.2/src/main.zig#L208) (208) | Implements shared CLI/config options, toggles, enum values, ranges, legacy spelling, repeated excludes, and file reads. Shared dispatch prevents precedence divergence. Translate into validated immutable ScanOptions and mutable browser preferences; record compression range discrepancy. |
| [tryReadArgsFile](../reference/ncdu-2.9.2/src/main.zig#L325) (325) | Missing/non-directory config paths are ignored, other I/O errors are fatal; 4096-byte buffered lines support comments and @ suppression. It avoids shell tokenization and expands selected config paths. Preserve semantics with BufRead and explicit line limits. |
| [version](../reference/ncdu-2.9.2/src/main.zig#L373) (373) | Writes the program version to stdout and exits successfully. Rust should report replacement/version identity honestly while keeping -v/-V compatibility; this is startup behavior, not a scan hot path. |
| [help](../reference/ncdu-2.9.2/src/main.zig#L378) (378) | Prints supported options and exits before session setup. Keep complete help synchronized with parser behavior and ncdu.1; the undocumented benchmark option still needs agent-facing documentation. |
| [readExcludeFile](../reference/ncdu-2.9.2/src/main.zig#L437) (437) | Streams nonempty lines into exclusion compilation without trimming them into a different pattern. Buffered reading saves syscalls and avoids a whole-file allocation. Preserve raw bytes and test CRLF/whitespace behavior. |
| [readImport](../reference/ncdu-2.9.2/src/main.zig#L449) (449) | Reads eight signature bytes, distinguishes indexed binary from raw/compressed JSON, and transfers binary descriptor ownership to the reader. This prevents format guessing by extension. Rust needs explicit seekability errors and reader ownership. |
| [main](../reference/ncdu-2.9.2/src/main.zig#L466) (466) | Loads locale/config/CLI, selects output and UI, resolves root, scans/imports, then drives refresh/shell/delete/browser states. Setup before scanning protects mode invariants; use a thin entry point plus Session methods so each branch can be tested. |
| [handleEvent](../reference/ncdu-2.9.2/src/main.zig#L629) (629) | Coordinates worker OOM notifications, rate-limited drawing, resize, and queued/blocking keys. It keeps curses on the main thread but is called frequently from scanning. Decouple progress polling from per-file work and preserve responsive cancellation. |
| [argument-parser test helper opt](../reference/ncdu-2.9.2/src/main.zig#L667) (667) | Checks token kind/value and Option.is in the parser regression fixture. It exists to make cluster/positional expectations explicit; retain behavioral cases in Rust tests rather than porting the helper literally. |
| [argument-parser test helper arg](../reference/ncdu-2.9.2/src/main.zig#L673) (673) | Checks values consumed after options, including attached and empty values. Together with opt it verifies parser cursor state; Rust test helpers can use descriptive expected-token records. |

### src/c.zig

This file declares no functions. Its c import is the entire C boundary. stdio supplies fopen/newterm; string supplies strerror; time supplies strftime/localtime; wchar supplies wcwidth; locale supplies setlocale/localeconv; fnmatch supplies wildcard matching; unistd supplies getuid; sys/types and pwd supply account lookup; Linux sys/vfs supplies fstatfs; curses and zstd supply UI/compression. _XOPEN_SOURCE exposes wcwidth. In Rust, split filesystem/account operations into os/linux.rs, terminal bindings into the terminal adapter, and compression into format code. Retain ABI ownership rules and avoid a single global unsafe catch-all.

Function coverage: no function declarations; all imports and feature definitions are analyzed above.

### src/scan.zig

The hot path combines metadata acquisition, cheap exclusion checks, descriptor-relative opens, and coarse directory scheduling. State has a bounded LIFO queue, atomic queue length, mutex/condition, thread slice, and waiting count. Dir owns iterator/descriptor/pattern state/sink; Thread owns a local DFS stack, sink, thread handle, number, and 4096-byte reusable name buffer. The fixed queue and small thread stacks are deliberate memory controls, not proven optimal tuning.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [isKernfs](../reference/ncdu-2.9.2/src/scan.zig#L15) (15) | Checks fstatfs type against twelve stable Linux kernel-filesystem magic values; errors return false. Device-crossing-only calls avoid an extra syscall per ordinary directory. Keep Linux-specific constants and explicit fail-open diagnostic policy in linux.rs. |
| [clamp](../reference/ncdu-2.9.2/src/scan.zig#L39) (39) | Uses the destination field type to saturate metadata conversion, avoiding duplicated range knowledge when field widths change. Rust uses typed conversion helpers rather than silently casting negative/oversize sizes. |
| [truncate](../reference/ncdu-2.9.2/src/scan.zig#L44) (44) | Uses the destination field type for bit truncation of identifiers and mode fields. This differs deliberately from clamping numerical sizes; Rust must keep these conversion policies distinct. |
| [statAt](../reference/ncdu-2.9.2/src/scan.zig#L49) (49) | Calls C fstatat directly because a source comment identifies a Zig 0.14 wrapper bug, maps errno, classifies directories before hardlinks/nonregular files, and fills all metadata/presence flags. Rust should preserve observations but need not retain a Zig workaround; measure statx versus fstatat. |
| [isCacheDir](../reference/ncdu-2.9.2/src/scan.zig#L89) (89) | Reads only the exact cache signature prefix, returning false on open/read errors and closing the file. Cheap bounded content verification prevents false exclusion by filename alone; preserve signature and descriptor cleanup. |
| [State.tryPush](../reference/ncdu-2.9.2/src/scan.zig#L121) (121) | Fast-rejects a full queue, then checks under lock, publishes one directory, and signals a waiter. Bounded handoff limits memory; Rust must publish initialized work under synchronization and execute overflow locally. |
| [State.waitPop](../reference/ncdu-2.9.2/src/scan.zig#L135) (135) | Waits on a condition while empty and ends when every worker is waiting. This works because active workers may still publish work. Use a documented outstanding-task invariant and cancellation wakeups, not queue-empty termination alone. |
| [Dir.create](../reference/ncdu-2.9.2/src/scan.zig#L163) (163) | Allocates a task combining open handle, device, compiled patterns, iterator, and sink. Ownership allows transfer between workers without rebuilding absolute paths; Rust can pool task allocations and use owned descriptors. |
| [Dir.destroy](../reference/ncdu-2.9.2/src/scan.zig#L175) (175) | Drops entered patterns, closes the descriptor, releases sink completion, then frees the task. The order ensures the sink observes fully enumerated descendants. Rust Drop/completion guards must handle errors and cancellation exactly once. |
| [Thread.scanOne](../reference/ncdu-2.9.2/src/scan.zig#L191) (191) | Applies any-kind exclusions before metadata, handles optional file-symlink targets and filesystem boundaries, opens directories no-follow, checks kernfs/cache exclusions, and dispatches observations/tasks. Its ordering avoids unnecessary syscalls and unsafe symlink traversal; reproduce it in focused stages with reused byte buffers. |
| [Thread.run](../reference/ncdu-2.9.2/src/scan.zig#L271) (271) | Takes queued tasks and processes a local DFS stack, updating progress/current directory and calling events on worker zero. Locality reduces shared queue traffic. Fix iterator errors targeting the outer dir, and avoid a sink mutex/current-path publication for every file if batching can preserve responsiveness. |
| [scan](../reference/ncdu-2.9.2/src/scan.zig#L297) (297) | Creates sink workers, stats/opens root before mutation, seeds the queue, spawns workers with 128 KiB stacks, runs worker zero locally, joins all threads, and finalizes the sink. Explicit join/finalization ensures totals are complete; Rust must cancel/join safely on setup/spawn failure. |

### src/model.zig

Memory layout and hardlink identity dominate this module. Entry is the common prefix; File carries only the inline name, Dir carries aggregates/relationships/device/error bits, Link carries a per-identity ring, and optional Ext is prepended. Ref is either pointer or binary itemref depending on global config: replace that mode-dependent union with typed tree access. Devices compress repeated u64 device IDs to u30 indices. Inodes uses 80%-load hash maps and a dirty subset/full-pass switch; these settings reduce repeated refresh work but must be measured in Rust.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [EType.base](../reference/ncdu-2.9.2/src/model.zig#L20) (20) | Maps special kinds and nonregular entries to the common File storage family while keeping Dir and Link distinct. Refresh can reuse allocation shapes despite kind changes; Rust should separate storage class from semantic kind. |
| [EType.isDirectory](../reference/ncdu-2.9.2/src/model.zig#L29) (29) | Treats real, other-filesystem, and kernel-filesystem entries as directory-like for display, even when excluded records use File storage. Keep display classification separate from traversability. |
| [Ref.isNull](../reference/ncdu-2.9.2/src/model.zig#L48) (48) | Uses null pointers for memory mode and u64::MAX for binary mode. Shared browser code motivates this union, but the global mode creates misuse risk; use explicit MemoryId versus BinaryRef APIs. |
| [Entry.dir](../reference/ncdu-2.9.2/src/model.zig#L78) (78) | Checks kind before casting the common prefix to Dir. Rust typed IDs or checked variant access achieve the same contract without reinterpreting packed memory. |
| [Entry.link](../reference/ncdu-2.9.2/src/model.zig#L82) (82) | Checks the hardlink kind before retrieving its larger record. Preserve the checked lookup and let hardlink-specific storage be absent for ordinary files. |
| [Entry.file](../reference/ncdu-2.9.2/src/model.zig#L86) (86) | Selects the compact record for every kind except Dir/Link. Special directory-like placeholders belong here too; do not inflate every entry to directory-sized storage. |
| [Entry.name](../reference/ncdu-2.9.2/src/model.zig#L90) (90) | Finds the type-dependent trailing NUL name without a separate pointer/length allocation. Rust byte-arena offsets/lengths retain compact ownership while avoiding sentinel scanning on repeated operations. |
| [Entry.nameHash](../reference/ncdu-2.9.2/src/model.zig#L100) (100) | Hashes raw name bytes with Wyhash for restoring browser selection. Hashing is a compact approximate identity; use collision-safe entry/name matching when refreshing and sorting. |
| [Entry.ext](../reference/ncdu-2.9.2/src/model.zig#L104) (104) | Retrieves optional metadata immediately before the record only if its flag is present. This saves overhead on the common nonextended scan; Rust side tables should separate allocated presence from current visibility. |
| [Entry.alloc](../reference/ncdu-2.9.2/src/model.zig#L109) (109) | Allocates exact bytes for optional Ext, concrete record and inline name, initializes fields, and retries OOM; adapts allocator alignment syntax across Zig versions. Keep compact intent, remove compiler-version glue and packed pointer arithmetic. |
| [Entry.create](../reference/ncdu-2.9.2/src/model.zig#L129) (129) | Dispatches semantic kinds to the smallest compatible allocation family. This is central to memory savings; a max-size Rust enum per file is not automatically equivalent. |
| [Entry.destroy](../reference/ncdu-2.9.2/src/model.zig#L137) (137) | Reconstructs allocation base and size, including optional prefix and trailing name, then frees it. Rust explicit owners/arenas avoid metadata-dependent deallocation; lazy directory arena and persistent scan model need separate lifetimes. |
| [Entry.hasErr](../reference/ncdu-2.9.2/src/model.zig#L148) (148) | Unifies direct special errors and directory direct/descendant flags for parent propagation. Use a small pure query with separate error categories so exclusion is not reported as an error. |
| [Entry.removeLinks](../reference/ncdu-2.9.2/src/model.zig#L154) (154) | Recursively detaches hardlink observations before subtree accounting changes. Ring contributions must be removed while old state is intact; Rust should use an iterative mutation traversal. |
| [Entry.zeroStatsRec](../reference/ncdu-2.9.2/src/model.zig#L162) (162) | Clears sizes, descendant counts and errors throughout a subtree after link removal. This supports in-place refresh/reuse; avoid recursion-depth failure and preserve old tree until root open succeeds. |
| [Entry.zeroStats](../reference/ncdu-2.9.2/src/model.zig#L178) (178) | Removes link contributions, subtracts subtree ordinary totals/items from ancestors, then resets recursively. Saturating subtraction avoids underflow but cannot recover previously clipped totals. Recompute affected aggregates where correctness requires it. |
| [Dir.fmtPath](../reference/ncdu-2.9.2/src/model.zig#L221) (221) | Collects ancestor names and appends them root-first, optionally omitting the root, without storing full paths per entry. Preserve on-demand byte path construction and explicit allocator ownership. |
| [Dir.updateSubErr](../reference/ncdu-2.9.2/src/model.zig#L242) (242) | Recomputes one directory's descendant-error flag assuming children are already updated. Bottom-up ordering is its invariant; refresh/deletion must propagate through ancestors afterward. |
| [Link.path](../reference/ncdu-2.9.2/src/model.zig#L275) (275) | Combines parent path and link basename for details/navigation. Calls allocate and caller frees; Rust should build display paths on demand and cache once when sorting many links. |
| [Link.addLink](../reference/ncdu-2.9.2/src/model.zig#L284) (284) | Inserts into the device/inode map, uncounts old contributions if needed, links the observation into a circular ring, and marks it dirty. Mutation-before-recount ordering prevents double counts; use explicit group records instead of self-referential pointers. |
| [Link.removeLink](../reference/ncdu-2.9.2/src/model.zig#L300) (300) | Uncounts prior stats, removes a lone group or repairs representative/ring pointers, then marks survivors dirty. It deliberately does not decrement filesystem nlink because refresh removal may not be real deletion. Distinguish observation removal from confirmed unlink in Rust. |
| [Ext.isEmpty](../reference/ncdu-2.9.2/src/model.zig#L344) (344) | Checks four presence bits, not numerical field values, so legitimate zero metadata stays distinguishable from missing data. Use compact presence flags with optional side storage. |
| [devices.getId](../reference/ncdu-2.9.2/src/model.zig#L359) (359) | Serializes interning device IDs, appending only unseen values and rejecting index overflow. Compact IDs save per-directory/link memory; worker-local caching can reduce repeated lock lookups if profiling shows contention. |
| [inodes.HashContext.hash](../reference/ncdu-2.9.2/src/model.zig#L392) (392) | Hashes compressed device ID and inode together, matching the true identity scope. A hash of inode alone would merge different filesystems; use an InodeKey type. |
| [inodes.HashContext.eql](../reference/ncdu-2.9.2/src/model.zig#L399) (399) | Compares inode and parent device identity even when map keys are different Link pointers. Equality is semantic, not address identity; Rust keys should carry these values directly. |
| [inodes.addUncounted](../reference/ncdu-2.9.2/src/model.zig#L404) (404) | Tracks dirty identities until the dirty map exceeds one eighth of the full map, then chooses a full recount. This trades memory/hash bookkeeping for sequential iteration; benchmark the crossover rather than hard-coding it as universally optimal. |
| [inodes.setStats](../reference/ncdu-2.9.2/src/model.zig#L417) (417) | Marks a whole link ring counted/uncounted, builds ancestor occurrence counts, resolves inconsistent nlink, and saturating-adds/removes total/shared contributions once per ancestor. This is the core accounting algorithm; parallelize with sharded identity work and a safe aggregate reduction, preserving external-link semantics. |
| [inodes.addAllStats](../reference/ncdu-2.9.2/src/model.zig#L477) (477) | Recounts full or dirty groups, emits progress, then resets dirty tracking. Deferred work makes enumeration simpler and refresh cheaper. Benchmark this phase separately and keep it inside end-to-end scan timing. |

### src/mem_sink.zig

Thread owns a page-backed arena for permanent entries; entries are deliberately not individually freed. Dir temporarily maps old child names for refresh reuse and collects child totals behind a mutex. Own sizes are tracked separately to avoid counting child directory metadata twice. global.root selects the refreshed subtree; global.stats disables aggregates for the multithreaded JSON staging tree.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [statToEntry](../reference/ncdu-2.9.2/src/mem_sink.zig#L20) (20) | Copies observation sizes/device/identity/extended data, sets parent relationships, and registers hardlinks under the global inode mutex. Centralization keeps import and scan consistent; Rust should batch identity registration and separate raw stat mapping from shared mutation. |
| [Dir.HashContext.hash](../reference/ncdu-2.9.2/src/mem_sink.zig#L57) (57) | Hashes an old entry's name to index refresh candidates without storing another name string. Rust borrowed byte-key lookups retain this allocation saving. |
| [Dir.HashContext.eql](../reference/ncdu-2.9.2/src/mem_sink.zig#L60) (60) | Uses pointer equality as a shortcut, then byte name equality. Semantically equal names can refer to different allocations; preserve raw byte equality and refresh type checks. |
| [Dir.HashContextAdapted.hash](../reference/ncdu-2.9.2/src/mem_sink.zig#L66) (66) | Hashes incoming name bytes identically to stored entries, enabling lookup before allocating a new record. Rust heterogeneous map lookup or a refresh index should do the same. |
| [Dir.HashContextAdapted.eql](../reference/ncdu-2.9.2/src/mem_sink.zig#L69) (69) | Compares incoming bytes with stored entry names, avoiding temporary key construction. Keep it consistent with the stored-key hash/equality. |
| [Dir.init](../reference/ncdu-2.9.2/src/mem_sink.zig#L74) (74) | Indexes current children with pre-reserved capacity and records the directory's own sizes. Existing children start as unseen candidates; capacity preallocation avoids repeated growth during refresh. |
| [Dir.getEntry](../reference/ncdu-2.9.2/src/mem_sink.zig#L93) (93) | Reuses unseen same-name entries of compatible base layout when extended storage suffices, otherwise prepends a new arena record. This avoids full rebuild cost; use generations and fix allocation-presence versus isext confusion instead of copying flags blindly. |
| [Dir.addSpecial](../reference/ncdu-2.9.2/src/mem_sink.zig#L109) (109) | Counts a visible placeholder, propagates errors, and obtains a zero-usage record. Exclusions must remain inspectable even though their contents are absent; preserve count semantics and use saturating/bounded counts consistently. |
| [Dir.addStat](../reference/ncdu-2.9.2/src/mem_sink.zig#L115) (115) | Counts one observation, adds ordinary sizes but defers hardlink sizes, updates latest subtree mtime, creates/reuses the entry, then maps stat data. This separates enumeration from hardlink reduction and prevents double counting. |
| [Dir.addDir](../reference/ncdu-2.9.2/src/mem_sink.zig#L132) (132) | Adds directory metadata once then initializes a child aggregation context. Parent own-directory size and descendant totals travel separately; preserve that distinction in completion records. |
| [Dir.setReadError](../reference/ncdu-2.9.2/src/mem_sink.zig#L136) (136) | Sets the directory's direct enumeration-error bit. Keep this independent of child errors and ensure worker iterator failures call the current context. |
| [Dir.final](../reference/ncdu-2.9.2/src/mem_sink.zig#L140) (140) | Unlinks unseen old children, frees the refresh index, combines child aggregates/errors/mtime, and merges into parent under one lock excluding child own sizes already counted there. Exactly-once child completion is essential; publish immutable completion batches in Rust. |
| [createRoot](../reference/ncdu-2.9.2/src/mem_sink.zig#L177) (177) | Allocates/selects root, zeros stats only after successful stat/open by the source, restores root own metadata/device/extended data, and initializes refresh tracking. This protects the prior view from a root-open failure; retain transactional refresh setup. |
| [done](../reference/ncdu-2.9.2/src/mem_sink.zig#L196) (196) | Updates refreshed subtree ancestor errors/totals and performs global hardlink recount. It skips all aggregation for JSON staging. Rust should explicitly distinguish staging from authoritative models and publish browse-ready results only after finalization. |

### src/mem_src.zig

A memory tree can act as another source. Ctx holds a sink worker and reusable stat value. This is why threaded JSON export can scan in parallel while preserving a single depth-first writer.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [toStat](../reference/ncdu-2.9.2/src/mem_src.zig#L12) (12) | Projects stored entries back into observations, retrieving devices and link counts/identity from parents and side records. Undefined device/inode values for irrelevant kinds are an internal shortcut; Rust initializes or uses typed variants so serializers cannot accidentally access absent fields. |
| [rec](../reference/ncdu-2.9.2/src/mem_src.zig#L34) (34) | Replays children depth-first, forwards own error flags and specials, maintains current-directory progress, and completes child sinks. Nesting order is required by JSON. Replace recursion with an explicit traversal stack and preserve directory own-size semantics of staging models. |
| [run](../reference/ncdu-2.9.2/src/mem_src.zig#L56) (56) | Creates one sink worker, reconstructs root path, replays child records, releases root, and finalizes. This reuses source/sink logic rather than a second exporter; keep ownership/finalization explicit and measure staging memory separately. |

### src/sink.zig

Stat is the compact transport observation (kind, u60 blocks, u64 size/dev/ino, u31 nlink, Ext). Dir wraps memory/JSON/binary sinks, atomic lifecycle count, parent and duplicated name. No concurrent direct methods on one Dir are permitted, but children may finalize on different workers. Thread has worker-specific output state, progress counters, and mutex-protected current directory; 32-bit targets lock u64 byte access. Global owns sink/state/error/quit confirmation. Rust should retain the lifecycle contract with owned session state and move UI rendering out of the hot sink boundary.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [Dir.addSpecial](../reference/ncdu-2.9.2/src/sink.zig#L80) (80) | Increments seen count, dispatches an exclusion/error record, and stores the last error path under lock. Path allocation occurs on errors, not every observation; preserve cheap normal behavior and bounded diagnostic state. |
| [Dir.addStat](../reference/ncdu-2.9.2/src/sink.zig#L98) (98) | Counts a file and adds approximate progress bytes divided by nlink before dispatching. The progress approximation avoids obvious duplicate inflation but is not final accounting; name it accordingly in Rust. |
| [Dir.addDir](../reference/ncdu-2.9.2/src/sink.zig#L109) (109) | Counts directory own usage, allocates/copies child context, dispatches concrete sink creation, and retains the parent. Retention protects parent aggregate state while child tasks outlive enumeration; pool directory contexts if measured useful. |
| [Dir.setReadError](../reference/ncdu-2.9.2/src/sink.zig#L129) (129) | Marks the concrete sink then replaces last-error path under lock. Enumeration failure preserves partial results; Rust error records should include origin/context without converting every failure to fatal exit. |
| [Dir.path](../reference/ncdu-2.9.2/src/sink.zig#L142) (142) | Walks parent contexts and builds a NUL-terminated path only when needed. This avoids per-file full paths; use byte path buffers and explicit ownership in Rust. |
| [Dir.ref](../reference/ncdu-2.9.2/src/sink.zig#L160) (160) | Relaxed-increments a lifecycle reference for a child. This is completion bookkeeping rather than arbitrary shared mutation; Rust task completion ownership may replace some refcount traffic. |
| [Dir.unref](../reference/ncdu-2.9.2/src/sink.zig#L164) (164) | Release-decrements, acquires on final release, finalizes output, recursively releases parent, then frees context/name. The acquire/release edge makes child writes visible before aggregation. Keep happens-before guarantees and use an iterative completion chain for deep trees. |
| [Thread.addBytes](../reference/ncdu-2.9.2/src/sink.zig#L194) (194) | Uses relaxed u64 atomic increments on wide targets and a mutex on 32-bit targets. Counters are observational; Rust batching can reduce cache traffic without changing final totals. |
| [Thread.getBytes](../reference/ncdu-2.9.2/src/sink.zig#L203) (203) | Loads progress under the same wide/32-bit policy as writes. Consistent synchronization avoids torn reads; do not add unnecessary global ordering for an approximate snapshot. |
| [Thread.setDir](../reference/ncdu-2.9.2/src/sink.zig#L212) (212) | Protects the current sink pointer so UI can copy its path before it is freed. Borrowed pointers outliving a lock are unsafe; Rust snapshots should hold owned IDs/path bytes under a clear lifetime. |
| [createThreads](../reference/ncdu-2.9.2/src/sink.zig#L232) (232) | Initializes global progress/worker sinks, clearing old diagnostics, and converts multiworker JSON to nonaggregating memory staging. This encodes sink capability restrictions; make capability selection explicit before workers start. |
| [done](../reference/ncdu-2.9.2/src/sink.zig#L255) (255) | Finalizes output, marks done, frees workers, optionally replays staged memory to JSON, and clears line progress. Recursive mode switching hides a second full phase; Rust sessions should expose scan/stage/write phases for measurements and cancellation. |
| [createRoot](../reference/ncdu-2.9.2/src/sink.zig#L275) (275) | Creates the root wrapper and concrete sink, with copied path and no parent. Root is not counted in files_seen; preserve that benchmark/count distinction. |
| [drawConsole](../reference/ncdu-2.9.2/src/sink.zig#L290) (290) | Aggregates worker counters, replaces ANSI progress lines when available, prints worker paths/waiting and hardlink-finalization counts. Rendering is rate-limited elsewhere; use stderr and sanitize raw paths. |
| [drawProgress](../reference/ncdu-2.9.2/src/sink.zig#L344) (344) | Renders terminal progress, limited worker rows, size/item counters, latest error and quit confirmation/animation. Workers provide snapshots; terminal owner controls frequency and layout. |
| [drawError](../reference/ncdu-2.9.2/src/sink.zig#L428) (428) | Shows failed refresh/root path and waits for a key. This is recoverable UI behavior distinct from fatal initial root failure; keep that session distinction. |
| [drawMessage](../reference/ncdu-2.9.2/src/sink.zig#L442) (442) | Draws a small generic message box but has no current callers. It is a removable helper, not a missing feature the port must invent. |
| [draw](../reference/ncdu-2.9.2/src/sink.zig#L450) (450) | Selects no UI, console, or curses and then dispatches scan/zeroing/hardlink/error states. Exhaustive Rust enums make unsupported state combinations clearer than globals. |
| [keyInput](../reference/ncdu-2.9.2/src/sink.zig#L481) (481) | Handles scan abort confirmation and return from refresh errors, while ignoring input during initialization/finalization. Preserve semantics initially and consider responsive cancellation during finalization as an explicit improvement. |

### src/exclude.zig

Pattern is an immutable linked sequence of slash-separated components with literal and directory-only flags. PatternList is specialized into terminal versus continuation rules, separating literal hash lookup from wildcard arrays. Continuation literals map to arrays because duplicate prefixes can have different suffixes; mutating shared pattern chains would undermine multithreading. root is absolute-root anchored; root_unanchored participates at every level. The tri-state result saves metadata syscalls for unconditional exclusions.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [Pattern.isLiteral](../reference/ncdu-2.9.2/src/exclude.zig#L49) (49) | Recognizes [, *, ?, and backslash as potential fnmatch syntax. Plain names get hash lookup instead of wildcard interpretation; preserve escaping/class behavior when using a crate. |
| [Pattern.parse](../reference/ncdu-2.9.2/src/exclude.zig#L57) (57) | Strips leading slashes, splits components into immutable linked nodes, handles slash-only suffixes, and marks intermediate/trailing-slash rules directory-only. The caller remembers anchoring separately. Rust compiled pattern IDs can reduce component allocations without changing grammar. |
| [PatternList](../reference/ncdu-2.9.2/src/exclude.zig#L123) (123) | Comptime-selects terminal-rule versus continuation map value types, eliminating runtime branching/storage for the unused variant. Rust may use two descriptive concrete structs; readability matters more than reproducing generic metaprogramming. |
| [PatternList.Ctx.hash](../reference/ncdu-2.9.2/src/exclude.zig#L137) (137) | Hashes component bytes rather than pointer identity so equivalent literals share one bucket. Keep borrowed names and patterns on the same hash scheme. |
| [PatternList.Ctx.eql](../reference/ncdu-2.9.2/src/exclude.zig#L140) (140) | Compares component bytes, allowing duplicate literal prefixes to coalesce. For continuation rules, keep all distinct suffixes rather than accidentally overwriting them. |
| [PatternList.append](../reference/ncdu-2.9.2/src/exclude.zig#L147) (147) | Adds literals to a map and wildcards to an array; any-kind exclusion wins over a directory-only duplicate; continuation suffixes accumulate. This both speeds matching and preserves precedence; test collisions/duplicate rule cases. |
| [PatternList.match](../reference/ncdu-2.9.2/src/exclude.zig#L163) (163) | Finds a literal result then scans fnmatch rules, stopping once an any-kind exclusion wins. The nullable bool encodes no match/any-kind/directory-only; use a named enum to make the Rust API readable. |
| [PatternList.enter](../reference/ncdu-2.9.2/src/exclude.zig#L173) (173) | Matches continuation prefixes and adds their suffix rules to the child state. It advances an automaton rather than reparsing absolute paths for every child, reducing hot traversal costs. |
| [PatternList.deinit](../reference/ncdu-2.9.2/src/exclude.zig#L178) (178) | Frees continuation arrays, map and wildcard references while leaving immutable pattern nodes intact. Rust owned compiled patterns plus lightweight per-directory state should avoid ambiguous leaks/ownership. |
| [Patterns.append](../reference/ncdu-2.9.2/src/exclude.zig#L196) (196) | Routes completed versus continuation components into the corresponding list. This makes match and enter independent operations; retain that separation in a compiled matcher. |
| [Patterns.match](../reference/ncdu-2.9.2/src/exclude.zig#L205) (205) | Combines local and unanchored terminal rules, giving any-kind exclusion precedence. Unanchored rules apply at every directory; do not substitute gitignore's slash anchoring. |
| [Patterns.enter](../reference/ncdu-2.9.2/src/exclude.zig#L214) (214) | Builds child-local continuation state from both current and unanchored prefixes. Immutable compiled rules permit safe worker sharing; bound/reuse transient child matcher storage where useful. |
| [Patterns.deinit](../reference/ncdu-2.9.2/src/exclude.zig#L221) (221) | Skips freeing the shared root, otherwise releases child state. Special ownership flag is a Zig convenience; Rust types should distinguish borrowed root state from owned entered state. |
| [addPattern](../reference/ncdu-2.9.2/src/exclude.zig#L237) (237) | Ignores empty rules, compiles a rule, and selects anchored versus unanchored roots from its leading slash. Preserve user-supplied bytes and repeated excludes. |
| [getPatterns](../reference/ncdu-2.9.2/src/exclude.zig#L247) (247) | Walks the absolute starting path to derive active anchored state; it is intentionally slower setup code, not the per-entry traversal API. Rust scan setup should use it once and enter child states incrementally. |
| [testfoo](../reference/ncdu-2.9.2/src/exclude.zig#L268) (268) | Reuses expected matches for an entered foo directory across test entry paths. It checks directory-only/any-kind precedence and wildcard continuations; migrate its cases, not its opaque helper name. |

### src/json_export.zig

Global holds one Writer because nesting is serial. Writer owns descriptor, optional ZstdWriter, 64 KiB reusable buffer, position, and the pending directory-object flag. ZstdWriter owns a C compression stream and bounded output buffer. Dir needs only device inheritance; streamed output does not retain an entire model in single-worker mode.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [ZstdWriter.create](../reference/ncdu-2.9.2/src/json_export.zig#L24) (24) | Allocates/initializes compression state, retries missing context on OOM, and sets requested level. Central ownership ensures one frame stream; Rust compressor construction should return a typed failure and record library/version policy. |
| [ZstdWriter.destroy](../reference/ncdu-2.9.2/src/json_export.zig#L40) (40) | Frees the C stream before the wrapper allocation. A Rust RAII encoder/context should release native resources even after output errors. |
| [ZstdWriter.write](../reference/ncdu-2.9.2/src/json_export.zig#L45) (45) | Feeds input until consumed, writes buffered output after a threshold, and loops until frame end on final flush. Partial processing is normal; Rust must finish the frame and propagate output/decompression errors distinctly. |
| [Writer.flush](../reference/ncdu-2.9.2/src/json_export.zig#L75) (75) | Checks an oversized required item, writes buffered bytes through optional compression, treats bytes==0 as final frame completion, and resets offset. Buffering reduces write calls; name finalization separately in Rust to avoid overloaded sentinel arguments. |
| [Writer.ensureSpace](../reference/ncdu-2.9.2/src/json_export.zig#L86) (86) | Flushes only when the pessimistic upcoming record exceeds remaining buffer capacity. This removes bounds checks from every emitted token; preserve checked capacity invariants before any unsafe optimization. |
| [Writer.write](../reference/ncdu-2.9.2/src/json_export.zig#L90) (90) | Copies a byte slice and advances buffer offset under the ensureSpace contract. Small helper keeps serializers readable; Rust safe extend/write APIs may optimize equally well. |
| [Writer.writeByte](../reference/ncdu-2.9.2/src/json_export.zig#L95) (95) | Appends one byte under the same reservation invariant. Avoid redundant allocation or trait-object calls per byte in hot encoding loops. |
| [Writer.writeStr](../reference/ncdu-2.9.2/src/json_export.zig#L101) (101) | Escapes control bytes/quotes/backslash/DEL while passing other bytes through, including invalid UTF-8. This is legacy byte-preserving export behavior; a String-based JSON serializer changes the contract. |
| [Writer.writeUint](../reference/ncdu-2.9.2/src/json_export.zig#L122) (122) | Formats decimal integers with two-digit chunks and a stack buffer, avoiding general/float formatting overhead. Benchmark a crate or standard integer formatter before writing a custom equivalent. |
| [Writer.init](../reference/ncdu-2.9.2/src/json_export.zig#L141) (141) | Creates optional compression and writes major/minor version plus program metadata/timestamp. Setup owns framing so per-item writers cannot omit it; avoid retaining the entire JSON document. |
| [Writer.closeDirEntry](../reference/ncdu-2.9.2/src/json_export.zig#L156) (156) | Closes a deferred directory object, optionally adding read_error before any child has been emitted. This is why late read errors can be lost; document and test the existing boundary or improve writer staging deliberately. |
| [Writer.writeSpecial](../reference/ncdu-2.9.2/src/json_export.zig#L164) (164) | Encodes a zero-usage record with read_error or exclusion reason; directory-like exclusions get an empty directory array. Wire shape matters to old readers, even though memory storage is compact File. |
| [Writer.writeStat](../reference/ncdu-2.9.2/src/json_export.zig#L179) (179) | Writes own sizes, changed device, hardlink identity/count, nonregular marker and present extended fields. Omitted zero sizes reduce output; absence flags for metadata must distinguish unknown from known zero. |
| [Dir.addSpecial](../reference/ncdu-2.9.2/src/json_export.zig#L227) (227) | Delegates special observations to the singleton writer. Serial depth-first source order makes no per-directory writer state necessary. |
| [Dir.addStat](../reference/ncdu-2.9.2/src/json_export.zig#L231) (231) | Closes pending directory metadata, emits one file object, and closes it. Device is irrelevant for ordinary file records; Rust typed observation handling should avoid undefined dummy values. |
| [Dir.addDir](../reference/ncdu-2.9.2/src/json_export.zig#L237) (237) | Closes prior pending metadata, starts a directory array/object, defers closing the object, and returns child device inheritance. Preserve balanced nesting and serial traversal. |
| [Dir.setReadError](../reference/ncdu-2.9.2/src/json_export.zig#L244) (244) | Asks the writer to close pending metadata with an error. It cannot modify already-emitted objects; explicit sink capability documentation prevents false expectations. |
| [Dir.final](../reference/ncdu-2.9.2/src/json_export.zig#L248) (248) | Reserves space, closes pending directory metadata, and closes its array. Exactly-once finalization preserves valid nesting on the successful path. |
| [createRoot](../reference/ncdu-2.9.2/src/json_export.zig#L255) (255) | Starts a directory with synthetic parent device zero. Reuses child encoding while still emitting root identity where required; cross-read root dev=0 fixtures. |
| [done](../reference/ncdu-2.9.2/src/json_export.zig#L260) (260) | Closes outer JSON array, finishes/writes output, destroys compression, closes descriptor, and frees Writer. Rust must report finish/flush failures rather than lose them in Drop. |
| [setupOutput](../reference/ncdu-2.9.2/src/json_export.zig#L268) (268) | Creates the singleton writer before observations arrive. Session ownership should replace a global pointer and make output capability selection explicit. |

### src/json_import.zig

A custom byte parser exists because legacy exports can contain invalid UTF-8 in JSON strings. Parser owns a 129 KiB buffer and line/byte counters, plus optional bounded zstd stream state. Ctx reuses one Stat and 32 KiB name buffer through DFS; it must save needed parent values before descending. This is streaming rather than DOM import; retain that memory property.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [ZstdReader.create](../reference/ncdu-2.9.2/src/json_import.zig#L19) (19) | Seeds compressed input with already-read signature bytes and creates a decompressor context with OOM retry. Consuming sniffed bytes exactly once is essential; Rust reader composition must not drop/replay the prefix. |
| [ZstdReader.destroy](../reference/ncdu-2.9.2/src/json_import.zig#L35) (35) | Releases native decompression state and wrapper. Use RAII to cover malformed stream/error paths too. |
| [ZstdReader.read](../reference/ncdu-2.9.2/src/json_import.zig#L40) (40) | Refills compressed bytes, decompresses until output exists, and rejects EOF while the frame remains incomplete. Streaming bounds memory; preserve concatenated frame/error behavior through fixtures. |
| [Parser.die](../reference/ncdu-2.9.2/src/json_import.zig#L74) (74) | Reports parser line/byte context and exits immediately. Cold error handling avoids hot-path allocation; Rust can return structured errors from a low-level parser and exit only at the app boundary. |
| [Parser.undoNextByte](../reference/ncdu-2.9.2/src/json_import.zig#L79) (79) | Pushes back a consumed delimiter by decrementing buffer/counter and rewriting its byte. It simplifies number scanning; Rust cursor-based lookahead can avoid fragile underflow and buffer-boundary assumptions. |
| [Parser.fill](../reference/ncdu-2.9.2/src/json_import.zig#L85) (85) | Refills from raw or compressed input and maps failures to readable parse diagnostics. Unified refill makes the tokenizer compression-agnostic; keep fatal I/O distinct from invalid syntax. |
| [Parser.nextByte](../reference/ncdu-2.9.2/src/json_import.zig#L98) (98) | Returns a byte with zero as EOF, refilling only at buffer exhaustion. Source comment reports an optional-byte variant slowed this implementation by about 30%; this is historical Zig evidence, not a Rust prediction. Use measured Rust cursor parsing and distinguish embedded NUL safely. |
| [Parser.nextChr](../reference/ncdu-2.9.2/src/json_import.zig#L110) (110) | Skips JSON whitespace and advances line counters, returning the next significant byte. Parser position matters for debugging corrupt imports; maintain it without path/string allocation per token. |
| [Parser.expectLit](../reference/ncdu-2.9.2/src/json_import.zig#L121) (121) | Checks a fixed tail such as true/false/null or a surrogate prefix. Shared literal checking keeps token validation consistent; bounds/EOF errors must be explicit in Rust. |
| [Parser.hexdig](../reference/ncdu-2.9.2/src/json_import.zig#L125) (125) | Decodes one hex nibble with uppercase/lowercase support or reports an invalid escape. Keep this small and testable as part of string decoding. |
| [Parser.stringContentSlow](../reference/ncdu-2.9.2/src/json_import.zig#L135) (135) | Handles escapes, surrogate pairs, arbitrary raw high bytes, EOF/control validation and bounded output while consuming excess content. Fast-path separation improves common strings. Rust must reject oversized names instead of silently creating aliasing paths and validate embedded NUL/path components. |
| [Parser.stringContent](../reference/ncdu-2.9.2/src/json_import.zig#L175) (175) | Copies the common unescaped string until a quote or special condition, then delegates slow decoding. This avoids full UTF-8 validation by design; preserve byte semantics and benchmark buffer scanning. |
| [Parser.string](../reference/ncdu-2.9.2/src/json_import.zig#L188) (188) | Requires a leading quote before content decoding. Separating delimiters from content supports callers that already consumed the quote. |
| [Parser.uintTail](../reference/ncdu-2.9.2/src/json_import.zig#L193) (193) | Parses unsigned decimal digits with pushback and a wrapping overflow heuristic. Fast decimal accumulation is useful, but the heuristic misses some overflows; replace with checked_mul/checked_add and boundary fixtures. |
| [Parser.uint](../reference/ncdu-2.9.2/src/json_import.zig#L209) (209) | Requires a digit then invokes typed accumulation. Rust bounded numeric conversion must reject negative/fractional values except the explicit mtime compatibility path. |
| [Parser.boolean](../reference/ncdu-2.9.2/src/json_import.zig#L216) (216) | Parses only true/false with literal tails. Presence/flags must not treat arbitrary nonempty tokens as true. |
| [Parser.obj](../reference/ncdu-2.9.2/src/json_import.zig#L224) (224) | Requires object opening delimiter. A small shared primitive keeps item and metadata parsing consistent. |
| [Parser.key](../reference/ncdu-2.9.2/src/json_import.zig#L228) (228) | Enforces object comma/first-key/closing delimiter rules and consumes the colon into a reusable key buffer. Avoid a HashMap of strings per record; oversized unknown keys must not alias recognized names through truncation. |
| [Parser.array](../reference/ncdu-2.9.2/src/json_import.zig#L245) (245) | Requires array opening delimiter. Directory nesting and top-level version framing both use it; a typed parsing stack can replace recursive context. |
| [Parser.elem](../reference/ncdu-2.9.2/src/json_import.zig#L249) (249) | Checks array separators/end and pushes the first content byte back when needed. Shared delimiter handling prevents missing/double comma acceptance in known fields. |
| [Parser.skipContent](../reference/ncdu-2.9.2/src/json_import.zig#L261) (261) | Skips unknown scalars/arrays/objects recursively, allowing future fields; number skipping is intentionally lax in source. Rust should preserve forward compatibility while enforcing safe syntax/depth/resource bounds. |
| [Parser.skip](../reference/ncdu-2.9.2/src/json_import.zig#L291) (291) | Obtains the next significant byte and delegates generic skipping. This keeps unknown metadata out of the model and avoids allocating discarded structures. |
| [Parser.eof](../reference/ncdu-2.9.2/src/json_import.zig#L295) (295) | Rejects non-whitespace trailing garbage after the outer document. Distinguish accepted additional outer-array elements from bytes after the array. |
| [itemkey](../reference/ncdu-2.9.2/src/json_import.zig#L373) (373) | Dispatches known keys by first byte then exact match, updates stat/presence/exclusion fields, accepts legacy names/unknown nlink/fractional mtime, and skips unknowns. Field-order interactions can affect kind; fixtures must vary order before adopting a general serde mapping. |
| [item](../reference/ncdu-2.9.2/src/json_import.zig#L483) (483) | Parses object or directory-array observations, requires a named root directory, inherits device, emits sink events, recursively walks children, rejects contents on excluded directories, and maintains progress. Rust uses an explicit stack and validates name/path semantics before any future delete/refresh action. |
| [import](../reference/ncdu-2.9.2/src/json_import.zig#L531) (531) | Creates one sink worker, chooses compressed/raw parser from magic, checks major version, skips metadata, imports root/trailing extension elements, verifies EOF, then finalizes. Preserve prefix bytes and bounded streaming memory; malformed input must return errors, not corrupt a partial model. |

### src/bin_export.zig

Global serializes physical writes and index allocation; each Thread encodes and compresses independent blocks outside that lock. Dir holds its stat, last child reference, ordinary/cumulative/shared counts, errors, mutex, and a per-device inode map. Names are raw CBOR byte strings. Block sizing starts at 64 KiB and grows across large block-number ranges to control index size; configured sizes override that. This backend supports multithreaded export without storing the full entry tree.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [bigu16](../reference/ncdu-2.9.2/src/bin_export.zig#L56) (56) | Produces big-endian two-byte encoding independent of host architecture. Rust to_be_bytes preserves portable wire representation. |
| [bigu32](../reference/ncdu-2.9.2/src/bin_export.zig#L57) (57) | Produces big-endian four-byte encoding for lengths/block IDs and compact integers. Keep conversions local to format code, not the OS backend. |
| [bigu64](../reference/ncdu-2.9.2/src/bin_export.zig#L58) (58) | Produces big-endian eight-byte encoding for index/large integer values. Stable byte order is required for cross-architecture imports. |
| [blockHeader](../reference/ncdu-2.9.2/src/bin_export.zig#L60) (60) | Combines four-bit kind and 28-bit length then encodes big-endian. Check range before packing in Rust; truncation can corrupt all following blocks. |
| [cborByte](../reference/ncdu-2.9.2/src/bin_export.zig#L62) (62) | Combines a CBOR major kind and five-bit argument. A format-specific encoder can remain small and readable without exposing bit arithmetic to domain code. |
| [blockSize](../reference/ncdu-2.9.2/src/bin_export.zig#L70) (70) | Chooses configured or increasing default uncompressed sizes as block numbers grow. This manages index count versus cache memory. Benchmark listing locality and enforce item/index 24-bit limits, not only the header limit. |
| [Thread.compressNone](../reference/ncdu-2.9.2/src/bin_export.zig#L95) (95) | Copies data uncompressed for debugging; it is unused in active output. Do not treat it as a user-visible uncompressed binary mode to preserve. |
| [Thread.compressZstd](../reference/ncdu-2.9.2/src/bin_export.zig#L100) (100) | Compresses one block with selected level and retries all errors as OOM. Compression outside global write lock enables parallelism; Rust must classify native errors rather than assuming every error means memory pressure. |
| [Thread.createBlock](../reference/ncdu-2.9.2/src/bin_export.zig#L108) (108) | Allocates a compression-bound output, writes header/block number/compressed payload/footer, and returns empty for unused buffers. Rust can reuse scratch output storage and check all packed length limits. |
| [Thread.flush](../reference/ncdu-2.9.2/src/bin_export.zig#L123) (123) | Compresses first, then locks physical output/index updates, fills offset/length index entry, allocates next logical block number, and resizes worker input. Short critical sections permit parallel compression. Avoid allocating a new empty indexed block at final flush if a simpler lifecycle can preserve format. |
| [Thread.cborHead](../reference/ncdu-2.9.2/src/bin_export.zig#L151) (151) | Uses the shortest unsigned argument-width encoding, from immediate to u64. Compact encoding reduces disk bytes and compression work; test every width boundary. |
| [Thread.cborIndef](../reference/ncdu-2.9.2/src/bin_export.zig#L174) (174) | Writes an indefinite-length marker for maps/break. It avoids counting all fields beforehand, simplifying optional output; preserve CBOR major validity. |
| [Thread.itemKey](../reference/ncdu-2.9.2/src/bin_export.zig#L179) (179) | Encodes stable numeric ItemKey as CBOR positive integer. Rust enum discriminants must not accidentally drift wire values. |
| [Thread.itemRef](../reference/ncdu-2.9.2/src/bin_export.zig#L183) (183) | Omits absent references and converts same-block backwards references to negative deltas because source says full references compress poorly. Validate monotonicity and block scope; semantic IDs and serialized refs are separate types. |
| [Thread.itemStart](../reference/ncdu-2.9.2/src/bin_export.zig#L194) (194) | Reserves a pessimistic whole-item bound, flushes before a split, captures itemref, and emits map/type/name/previous sibling. No item spans blocks, enabling lazy reads; Rust checks oversized names and keeps byte-string encoding. |
| [Thread.itemExt](../reference/ncdu-2.9.2/src/bin_export.zig#L211) (211) | Emits only present fields when extended export is enabled. Known-zero metadata still gets encoded; data presence must not be inferred from value. |
| [Thread.itemEnd](../reference/ncdu-2.9.2/src/bin_export.zig#L231) (231) | Writes CBOR break to close an indefinite item map. Balanced item framing is a testable wire invariant. |
| [Dir.addSpecial](../reference/ncdu-2.9.2/src/bin_export.zig#L270) (270) | Locks directory linkage/counters, counts a special entry, propagates suberror, and encodes it into the worker block. Sibling chain order can vary across workers without changing contents; normalized comparisons must ignore order. |
| [Dir.addStat](../reference/ncdu-2.9.2/src/bin_export.zig#L279) (279) | Counts ordinary sizes or registers hardlink inode occurrence, emits own size/identity/extended metadata, and updates last child. Per-dir dedup is required for local totals; device scope comes from the directory. |
| [Dir.addDir](../reference/ncdu-2.9.2/src/bin_export.zig#L311) (311) | Counts child directory own sizes immediately and returns its independent stat/context. Descendant totals arrive later; retain separate own versus cumulative values to avoid double counting. |
| [Dir.setReadError](../reference/ncdu-2.9.2/src/bin_export.zig#L320) (320) | Sets direct directory failure under lock, allowing cross-worker completion safely. Keep direct and descendant errors distinct in serialization. |
| [Dir.countLinks](../reference/ncdu-2.9.2/src/bin_export.zig#L333) (333) | Adds each inode once locally, marks sharing when not all links are found, drops fully contained groups after parent transfer, and moves/merges remaining maps. This bounds retained identities during streaming; unknown nlink lacks shared accounting and device boundaries prohibit inode-only map merge. |
| [Dir.final](../reference/ncdu-2.9.2/src/bin_export.zig#L377) (377) | Locks parent, accumulates descendants/errors, handles same/different-device hardlink merges, and emits completed directory with own/cumulative/shared counts and child references. Postorder encoding enables low-memory streaming and lazy browse; ensure parent completion cannot precede this write. |
| [createRoot](../reference/ncdu-2.9.2/src/bin_export.zig#L434) (434) | Allocates initial input buffers for every worker and returns root context. Memory grows with worker count even without a tree; include compression scratch/contexts in RSS budgets. |
| [done](../reference/ncdu-2.9.2/src/bin_export.zig#L442) (442) | Flushes workers, frees buffers, removes trailing empty index entries, writes root reference/index/footer, then closes output. Final index publication makes the file browsable; Rust must surface final write failures and avoid interpreting interrupted exports as complete. |
| [setupOutput](../reference/ncdu-2.9.2/src/bin_export.zig#L460) (460) | Writes signature and initializes physical offset plus index header placeholder. Framing is set before worker writes; Rust output session should own this state rather than rely on globals. |

### src/bin_reader.zig

The implemented mode is indexed random access, with eight cached decompressed Block values and a monotonically increasing LRU timestamp. Global owns fd/index/cache/last-item error context. CborReader borrows one block; CborVal references its cursor; ItemParser tracks definite/indefinite maps. Import replays DFS into sink; get materializes only one entry. Borrowed buffers become invalid when reads replace cache state, so Rust lifetimes/copies must enforce that boundary.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [bigu16](../reference/ncdu-2.9.2/src/bin_reader.zig#L61) (61) | Decodes big-endian two-byte integers. Use from_be_bytes with checked slices to avoid alignment assumptions. |
| [bigu32](../reference/ncdu-2.9.2/src/bin_reader.zig#L62) (62) | Decodes block/index header words independently of machine endian. Validate kind and length before allocating. |
| [bigu64](../reference/ncdu-2.9.2/src/bin_reader.zig#L63) (63) | Decodes index entries/root references and CBOR integers. Keep wire arithmetic checked and distinct from saturating usage totals. |
| [die](../reference/ncdu-2.9.2/src/bin_reader.zig#L65) (65) | Reports last item reference when known, otherwise a generic read failure, and marks diagnostics cold. Rust structured errors can add context without hot-path string allocation. |
| [readBlock](../reference/ncdu-2.9.2/src/bin_reader.zig#L72) (72) | Linear-searches eight cached blocks, updates LRU, evicts oldest, reads compressed bytes with pread, bounds declared content, and decompresses. Small cache makes linear lookup reasonable; Rust should validate envelopes/index ranges and cap bytes, not merely block count when configured blocks are large. |
| [CborReader.head](../reference/ncdu-2.9.2/src/bin_reader.zig#L118) (118) | Consumes CBOR major/argument widths and permitted indefinite markers with bounds checks. This small decoder avoids a full CBOR object tree; preserve strict slicing and all integer boundaries. |
| [CborReader.next](../reference/ncdu-2.9.2/src/bin_reader.zig#L164) (164) | Skips semantic tags and returns the next underlying value. Forward-compatible tags need no model representation; avoid unbounded tag loops on hostile input. |
| [CborVal.end](../reference/ncdu-2.9.2/src/bin_reader.zig#L178) (178) | Recognizes indefinite simple break marker. Shared terminator query keeps nested/map decoding consistent. |
| [CborVal.int](../reference/ncdu-2.9.2/src/bin_reader.zig#L182) (182) | Converts positive/negative CBOR arguments to requested bounded integer types, rejecting overflow. Rust checked conversions must handle i64::MIN correctly rather than negating it unsafely. |
| [CborVal.isTrue](../reference/ncdu-2.9.2/src/bin_reader.zig#L194) (194) | Recognizes CBOR simple value 21 as true. Existing other values are treated false; malformed boolean handling should be deliberately specified, not accidentally broadened. |
| [CborVal.bytes](../reference/ncdu-2.9.2/src/bin_reader.zig#L200) (200) | Borrows definite bytes/text without UTF-8 validation, rejecting indefinite strings and short buffers. Raw name support is intentional; copy before block eviction and validate operation-safe path components. |
| [CborVal.skip](../reference/ncdu-2.9.2/src/bin_reader.zig#L208) (208) | Skips unknown definite/indefinite containers with some element bounds, recursively. This supports future fields; add depth/total-work bounds and iterative state where needed. |
| [CborVal.etype](../reference/ncdu-2.9.2/src/bin_reader.zig#L234) (234) | Maps known signed values to kinds and unknown negatives/positives to excluded/nonregular fallbacks. This is forward compatibility; keep wire classification explicit and avoid invalid Rust enum transmute. |
| [CborVal.itemref](../reference/ncdu-2.9.2/src/bin_reader.zig#L240) (240) | Resolves absolute refs or same-block negative backwards deltas, rejecting underflow/cross-block deltas. This reverses writer compression; also validate referenced records/cycles before traversal. |
| [ItemParser.init](../reference/ncdu-2.9.2/src/bin_reader.zig#L319) (319) | Requires an item map and tracks its optional definite field count. Compatibility allows definite maps even though current writer emits indefinite ones. |
| [ItemParser.key](../reference/ncdu-2.9.2/src/bin_reader.zig#L327) (327) | Advances one definite entry or stops at an indefinite break. Map count/end handling must remain consistent through unknown-value skipping. |
| [ItemParser.next](../reference/ncdu-2.9.2/src/bin_reader.zig#L339) (339) | Returns numeric keys that fit ItemKey width, skipping other key/value pairs. Known-width unknown keys are handled later; retain forward compatibility with safe work limits. |
| [readItem](../reference/ncdu-2.9.2/src/bin_reader.zig#L354) (354) | Sets error context, validates 56-bit reference/offset, obtains block, and creates a map parser. Returned borrow expires at subsequent reads; Rust should make block guard ownership visible. |
| [Import.readFields](../reference/ncdu-2.9.2/src/bin_reader.zig#L375) (375) | Loads own stat/name/error/prev/sub/identity/extended presence and skips cumulative fields because the sink recomputes them. Name and kind are required; verify duplicate/order/embedded-NUL policy and block lifetimes. |
| [Import.import](../reference/ncdu-2.9.2/src/bin_reader.zig#L404) (404) | DFS-replays a directory and its previous-sibling child chain, saving parent continuation and inherited device around recursion. JSON needs this order. Rust explicit frames must copy saved borrowed state before fetching another block. |
| [get](../reference/ncdu-2.9.2/src/bin_reader.zig#L439) (439) | Parses twice: first kind/name/metadata to allocate the right record, then aggregate/link/reference fields. Two passes avoid a temporary large record; materialization must distinguish binary references from in-memory parent/ring fields left unset. |
| [getRoot](../reference/ncdu-2.9.2/src/bin_reader.zig#L488) (488) | Reads the final eight index bytes as root itemref. Require enough index bytes and a valid directory root before exposing browsing. |
| [import](../reference/ncdu-2.9.2/src/bin_reader.zig#L496) (496) | Creates a single sink worker and DFS-replays the indexed root then finalizes. This is not a sequential stream importer; keep seekability and conversion performance explicit. |
| [open](../reference/ncdu-2.9.2/src/bin_reader.zig#L504) (504) | Seeks to EOF rather than using Zig getEndPos to support older kernels lacking statx, reads footer/index, and stores fd. Rust must retain a fallback path and validate footer lengths against file size before allocating or subtracting. |

### src/browser.zig

This is the largest source file. Globals own current parent/path, parent reference stack, a resettable lazy-entry arena, ptr-to-file-ref lookup, visible sorted items, graph maxima, loading counter, cursor, remembered views, dialogs/messages. View stores scroll/hashed selection by directory path. Row renders independent columns. Quit/info/help own dialog state. Rust should use BrowserState plus a TreeAccess abstraction, preserve lazy reads, and avoid global mode-dependent pointers.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [View.dirHash](../reference/ncdu-2.9.2/src/browser.zig#L47) (47) | Hashes current path to key remembered directory views. This saves storing each whole path but admits collisions; Rust can use stable directory identities or verified path keys. |
| [View.save](../reference/ncdu-2.9.2/src/browser.zig#L52) (52) | Captures selected name hash and saves scroll/view state, ignoring allocation failure. This preserves navigation position with minimal bookkeeping; use fallible bounded view history and collision-safe identity. |
| [View.load](../reference/ncdu-2.9.2/src/browser.zig#L60) (60) | Restores saved state and selects a requested or remembered name hash after listing/sort changes. Names rather than old array indices survive reordered listings; prefer entry/name identity with defined fallback when removed. |
| [sortIntLt](../reference/ncdu-2.9.2/src/browser.zig#L80) (80) | Returns no decision on equality, otherwise compares according to configured direction. This enables ordered secondary criteria; Rust Ordering chaining expresses it clearly. |
| [sortLt](../reference/ncdu-2.9.2/src/browser.zig#L84) (84) | Applies optional directory-first grouping, primary/secondary size/count/mtime keys, then natural/raw name tie-breaking in configured order. Preserve deterministic comparison and metadata-absent ordering; test natural comparator consistency. |
| [sortDir](../reference/ncdu-2.9.2/src/browser.zig#L124) (124) | Excludes synthetic parent row from sorting, sorts children, and restores selection. Parent navigation remains pinned regardless of sort direction; Rust listing type can represent it explicitly. |
| [loadDir](../reference/ncdu-2.9.2/src/browser.zig#L136) (136) | Resets lazy arena/listing refs, walks memory or binary sibling refs, calculates graph maxima/shared presence, applies hidden filter, publishes loading progress, and sorts. Hidden entries still influence maxima in current code; capture that behavior before any UI correction. Avoid eagerly materializing the whole tree. |
| [initRoot](../reference/ncdu-2.9.2/src/browser.zig#L184) (184) | Loads binary root on demand or selects memory root, seeds parent stack and path, then loads its children. Root ownership differs from listing arena; use explicit owners so resetting listing cannot invalidate current directory. |
| [enterSub](../reference/ncdu-2.9.2/src/browser.zig#L197) (197) | Resolves binary directory ref or switches memory parent, pushes navigation stack, and extends current path. Borrowed child e/name can belong to listing arena; keep path construction before invalidation and descriptor-free navigation. |
| [enterParent](../reference/ncdu-2.9.2/src/browser.zig#L216) (216) | Pops navigation stack, reloads binary parent if needed, and removes last path component. Root boundary is asserted; Rust navigation API should reject invalid upward movement safely. |
| [Row.flag](../reference/ncdu-2.9.2/src/browser.zig#L241) (241) | Displays direct/descendant errors, empty dirs, hardlinks, exclusions, and nonregular types while reserving two columns. These symbols communicate incomplete data; preserve their meanings and test error entries that currently lack a flag. |
| [Row.size](../reference/ncdu-2.9.2/src/browser.zig#L261) (261) | Renders selected apparent/allocated metric plus shared or unique directory column using saturating difference. Shared column appears only when directory listing contains sharing; retain metric units and column layout. |
| [Row.graph](../reference/ncdu-2.9.2/src/browser.zig#L280) (280) | Renders optional percent/bar with overflow-aware integer scaling and hash/half/eighth block glyphs. Ratio denominators and narrow-screen guards prevent divide-by-zero/overflow; use named scaling helpers with boundary fixtures. |
| [Row.items](../reference/ncdu-2.9.2/src/browser.zig#L330) (330) | Displays descendant counts only for directories with compact decimal/k/M/>1G formatting when space allows. Counts are descendants, not only immediate children; keep that distinction in column naming/help. |
| [Row.mtime](../reference/ncdu-2.9.2/src/browser.zig#L361) (361) | Displays available latest subtree mtime or missing indicator and skips the column on narrow terminals. Presence flags matter; zero must not mean absent. |
| [Row.name](../reference/ncdu-2.9.2/src/browser.zig#L375) (375) | Draws directory marker and sanitized/shortened byte name, or synthetic /.. parent row. Display text must never be reused as the filesystem path. |
| [Row.draw](../reference/ncdu-2.9.2/src/browser.zig#L387) (387) | Clears selected row then renders columns in order with a moving column cursor. Composition makes adding a column local; Rust can use a render layout rather than monolithic drawing logic. |
| [quit.draw](../reference/ncdu-2.9.2/src/browser.zig#L406) (406) | Shows a compact confirmation prompt using semantic key/style helpers. Preserve default cancellation and terminal-size handling. |
| [quit.keyInput](../reference/ncdu-2.9.2/src/browser.zig#L420) (420) | Confirms only y/Y; all other input returns to main browser. This intentional conservative interaction should remain separate from deletion confirmation. |
| [info.lt](../reference/ncdu-2.9.2/src/browser.zig#L437) (437) | Builds two link paths on every comparator call and compares bytes. Sorting links by path makes them navigable; Rust can precompute paths once to avoid O(n log n) repeated allocations. |
| [info.set](../reference/ncdu-2.9.2/src/browser.zig#L446) (446) | Resets cached link list when selection changes, opens info/links tab, gathers one inode ring, sorts paths, and selects the current observation. Lazy binary mode cannot enumerate global link groups; retain that capability restriction. |
| [info.drawLinks](../reference/ncdu-2.9.2/src/browser.zig#L475) (475) | Scrolls link paths, highlights selection/current link, and displays count; file-backed mode explains unsupported functionality. Use valid raw-path identity and separate sanitized display. |
| [info.drawSizeRow](../reference/ncdu-2.9.2/src/browser.zig#L501) (501) | Prints both human-readable and exact byte values. Exact values let users verify rounded display; keep shared formatting in one helper. |
| [info.drawSize](../reference/ncdu-2.9.2/src/browser.zig#L511) (511) | Prints total, optional shared, and saturating unique values with consistent styles. Reuse it for apparent/allocated metrics so semantics cannot diverge. |
| [info.drawInfo](../reference/ncdu-2.9.2/src/browser.zig#L521) (521) | Displays name, type or extended mode/UID/GID, optional mtime, both size metrics, descendant count and hardlink identity/count. Known-zero metadata is visible; model and renderer should expose presence explicitly. |
| [info.draw](../reference/ncdu-2.9.2/src/browser.zig#L604) (604) | Chooses dynamic dialog height, tabs and content then close hint. Existing fixed width/dynamic height can clip on small terminals; test resize and improve clipping intentionally. |
| [info.keyInput](../reference/ncdu-2.9.2/src/browser.zig#L644) (644) | Switches tabs, scrolls links or selection, jumps to selected hardlink parent, and closes info. The jump does not rebuild path/parent stack in current source; Rust navigation must update all components atomically. |
| [help.drawKeys](../reference/ncdu-2.9.2/src/browser.zig#L722) (722) | Draws ten key-description rows from a scroll offset and indicates more. Centralize the binding table so future features update help without duplicate strings. |
| [help.drawFlags](../reference/ncdu-2.9.2/src/browser.zig#L740) (740) | Explains listing flag symbols and layout. Preserve error/exclusion distinctions; fixtures can keep renderer symbols and explanations aligned. |
| [help.drawAbout](../reference/ncdu-2.9.2/src/browser.zig#L758) (758) | Draws the historical bitmapped text logo, version, attribution and URL. It is presentation/credit, not a performance requirement; preserve original copyright alongside honest rewrite identity. |
| [help.draw](../reference/ncdu-2.9.2/src/browser.zig#L778) (778) | Draws three help tabs and close hint, delegating the active tab. Separate tab state keeps help independent from main navigation. |
| [help.keyInput](../reference/ncdu-2.9.2/src/browser.zig#L798) (798) | Changes tabs/scrolls within key limits and resets offset when leaving or switching. Saturating bounds prevent out-of-range table access; preserve arrow/vim/page bindings. |
| [draw](../reference/ncdu-2.9.2/src/browser.zig#L821) (821) | Renders header/mode marker, current path, visible rows, aggregate footer, dialogs/messages and selected cursor position; uses a loading view while lazy entries arrive. The first browse frame and sorting are distinct benchmark phases; retain valid behavior for tiny/empty terminals. |
| [sortToggle](../reference/ncdu-2.9.2/src/browser.zig#L910) (910) | Chooses a default order when switching columns and flips direction on repeated selection, then sorts. Key defaults can differ from CLI defaults (notably mtime); capture both in tests. |
| [keyInputSelection](../reference/ncdu-2.9.2/src/browser.zig#L918) (918) | Handles vim/arrows/home/end/page keys with saturated bounds and returns whether input belonged to navigation. Reuse between listing/details to avoid divergent edge handling. |
| [keyInput](../reference/ncdu-2.9.2/src/browser.zig#L935) (935) | Routes dialogs, gates shell/delete/refresh capabilities, toggles sort/filter/size/graph/shared display, navigates dirs, and saves view afterward. Exhaustive actions and explicit BrowserState make adding features safer; never run destructive actions on synthetic parent rows or lazy imported refs. |

### src/delete.zig

Deletion is an interactive state machine with selected parent/entry/next selection, confirm/busy/error phase, yes/no/ignore confirmation, and abort/ignore/all error choices. Built-in traversal acts on the scanned model, so new unseen children can prevent rmdir; it does not blindly enumerate/delete newly appeared content. Custom command receives NCDU_DELETE_PATH through the environment and a quoted variable reference. These behaviors must be tested only on disposable fixtures.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [setup](../reference/ncdu-2.9.2/src/delete.zig#L23) (23) | Captures target and post-delete selection, chooses confirmation or busy state, and defaults answer to no. Keep target identity/path stable so UI changes cannot redirect a pending action. |
| [err](../reference/ncdu-2.9.2/src/delete.zig#L33) (33) | Skips ignored errors or enters an interactive error state until user decides, returning whether deletion should abort. Partial deletion is normal; preserve surviving records and report actual I/O failures. |
| [deleteItem](../reference/ncdu-2.9.2/src/delete.zig#L45) (45) | Walks scanned children via no-follow directory opens, supports cancellation, unlinks files/rmdirs after children, then zeros stats and removes successful records. It avoids following replacement symlink directories and skips unlisted new children; Rust explicit stack/descriptor ownership must retain partial-result semantics. |
| [deleteCmd](../reference/ncdu-2.9.2/src/delete.zig#L75) (75) | Runs configured shell command with quoted environment path, restats afterward, replaces changed non-directory records or schedules directory refresh, then updates stats. Treat only verified disappearance as success; other stat failures must not erase model data. |
| [delete](../reference/ncdu-2.9.2/src/delete.zig#L126) (126) | Waits for confirmation, locates mutable sibling slot, builds target path, selects built-in/custom strategy, recounts hardlinks, and returns next selection. Model mutation and accounting must be coordinated, with no live references invalidated during removal. |
| [drawConfirm](../reference/ncdu-2.9.2/src/delete.zig#L157) (157) | Shows target or custom command, yes/no/don't-ask options, and positions cursor on choice. Sanitized shortened text is for review; actual raw path remains separately owned. |
| [drawProgress](../reference/ncdu-2.9.2/src/delete.zig#L199) (199) | Shows current model path and abort key during recursive work. UI polling must remain responsive without sending one expensive path snapshot per entry. |
| [drawErr](../reference/ncdu-2.9.2/src/delete.zig#L219) (219) | Shows failed target/OS error and abort/ignore/ignore-all selection. This preserves user control over partial deletion; keep error code/context available until action resolution. |
| [draw](../reference/ncdu-2.9.2/src/delete.zig#L246) (246) | Dispatches confirm/busy/error rendering. A single state enum ensures UI and deletion operation agree on phase. |
| [keyInput](../reference/ncdu-2.9.2/src/delete.zig#L254) (254) | Changes choices with arrows/vim keys, cancels on q, starts deletion on confirmation, and updates session-wide confirmation/error policies. Test conservative defaults and explicit session-only don't-ask/ignore-all behavior. |

### src/ui.zig

Terminal helpers own initialization state, main-thread identity, OOM counter, dimensions, reusable display buffers, style tables, generated style enum, and box positioning. Ncurses locale/wide-character behavior is coupled to libc wcwidth. All UI calls must remain on one thread. Three themes and background/foreground combinations are presentation state, not filesystem model fields.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [die](../reference/ncdu-2.9.2/src/ui.zig#L18) (18) | Restores the terminal, prints stderr diagnostic and exits failure. Rust core modules should return errors and let the process boundary do cleanup/output. |
| [quit](../reference/ncdu-2.9.2/src/ui.zig#L24) (24) | Restores terminal and exits success immediately. An explicit session shutdown also joins workers/releases descriptors; avoid process exit from reusable library APIs. |
| [oom](../reference/ncdu-2.9.2/src/ui.zig#L40) (40) | Main thread temporarily resets terminal and retries after a second; workers increment an OOM indicator and sleep. This gives interactive recovery but may retry forever. Rust should define recoverable allocation boundaries and cancel cleanly where standard allocations cannot be recovered. |
| [errorString](../reference/ncdu-2.9.2/src/ui.zig#L58) (58) | Maps Zig I/O errors to user-readable strings because errno isn't uniformly exposed. Rust io::Error supplies OS context; preserve distinctions rather than adopting the incomplete mapping blindly. |
| [toUtf8BadChar](../reference/ncdu-2.9.2/src/ui.zig#L85) (85) | Marks ASCII controls/DEL unsafe for display. Sanitization helps filenames avoid terminal control effects; consider unsafe Unicode controls as an explicit additional policy. |
| [toUtf8](../reference/ncdu-2.9.2/src/ui.zig#L98) (98) | Returns original valid printable UTF-8 or constructs a reusable buffer with invalid/control bytes rendered as \xHH. Raw names survive unchanged elsewhere. Rust should return borrowed/owned display text with no global buffer aliasing. |
| [shorten](../reference/ncdu-2.9.2/src/ui.zig#L129) (129) | Computes terminal-column width with wcwidth and preserves prefix/suffix around ellipsis. Byte or character counts are insufficient for wide glyphs; retain width correctness and consciously improve combining/grapheme handling. |
| [shortenTest](../reference/ncdu-2.9.2/src/ui.zig#L173) (173) | Compares shortening to expected output in fixtures. Preserve wide/combining/variation-selector edge cases and document deliberate visual corrections. |
| [StyleDef.style](../reference/ncdu-2.9.2/src/ui.zig#L202) (202) | Selects theme-specific foreground/background/attributes. Keep semantic style names so domain features do not hard-code colors. |
| [Bg.fg](../reference/ncdu-2.9.2/src/ui.zig#L306) (306) | Maps a semantic foreground to its default/header/selected theme variant. This avoids duplicating combinations in every row; a Rust style palette can be explicit rather than generated through type reflection. |
| [updateSize](../reference/ncdu-2.9.2/src/ui.zig#L329) (329) | Reads curses rows/columns through functions because Zig cannot translate the preferred macro. Rust terminal adapter should hide binding quirks and expose safe dimensions. |
| [clearScr](../reference/ncdu-2.9.2/src/ui.zig#L335) (335) | Clears leftover line-progress output with an ANSI clear-to-end sequence. UI-mode transitions explain this extra write; avoid unconditionally polluting benchmark/export stderr when no terminal cleanup is needed. |
| [init](../reference/ncdu-2.9.2/src/ui.zig#L341) (341) | Opens /dev/tty/newterm when standard streams carry data, otherwise initscr, sets cbreak/noecho/hidden cursor/keypad, initializes colors, and marks ready. Separate UI channel from export streams and use a terminal guard for partial setup failure. |
| [deinit](../reference/ncdu-2.9.2/src/ui.zig#L366) (366) | Clears/refreshes/endwin or clears leftover console output, then marks inactive. Idempotent cleanup supports normal/error/panic/child-command paths; test terminal restoration through PTY. |
| [style](../reference/ncdu-2.9.2/src/ui.zig#L377) (377) | Applies palette attributes/color-pair index. Centralizing binding calls keeps rendering code independent of curses internals. |
| [move](../reference/ncdu-2.9.2/src/ui.zig#L381) (381) | Converts unsigned logical coordinates to curses signed values. Rust clipping/checked conversion should avoid oversized/narrow-terminal arithmetic faults. |
| [addstr](../reference/ncdu-2.9.2/src/ui.zig#L388) (388) | Sends NUL-terminated text to curses, with noted line-wrapping limitations. Validate/sanitize display text and do not let raw embedded NUL silently truncate operation paths. |
| [addprint](../reference/ncdu-2.9.2/src/ui.zig#L393) (393) | Formats into a 256-byte stack buffer and sends it to addstr. Fixed buffer avoids heap allocation for predictable small UI text; Rust should return/clip safely if output can exceed assumptions. |
| [addch](../reference/ncdu-2.9.2/src/ui.zig#L399) (399) | Adds a curses character/glyph. Keep adapter ownership and distinguish byte chars from Unicode terminal cells. |
| [FmtSize.init](../reference/ncdu-2.9.2/src/ui.zig#L411) (411) | Rounds a scaled integer and formats fixed five-character decimal text without floats. Integer math avoids large formatting tables and defines stable rounding; test overflow near maxima. |
| [FmtSize.fmt](../reference/ncdu-2.9.2/src/ui.zig#L418) (418) | Selects SI or binary units using precise thresholds up to EB/EiB. This avoids rounding to 1000 in the old unit; preserve threshold fixtures instead of a loosely equivalent generic human-size crate. |
| [FmtSize.num](../reference/ncdu-2.9.2/src/ui.zig#L440) (440) | Exposes the formatted numeric buffer separate from unit for independent styles. A small value object avoids transient heap strings. |
| [FmtSize.testEql](../reference/ncdu-2.9.2/src/ui.zig#L444) (444) | Combines numeric/unit portions and checks expected strings. Boundary tests are part of UI compatibility and integer safety. |
| [addsize](../reference/ncdu-2.9.2/src/ui.zig#L489) (489) | Styles number and unit separately using FmtSize. Shared implementation keeps scanner/browser/details consistent. |
| [addnum](../reference/ncdu-2.9.2/src/ui.zig#L500) (500) | Formats full decimal digits and inserts locale thousands separator into fixed buffers. Grouping assumes three digits, not full locale grouping rules; bound multibyte separators safely in Rust. |
| [addmode](../reference/ncdu-2.9.2/src/ui.zig#L522) (522) | Displays type and permission/special bits in ten columns. Preserve mode semantics, then deliberately correct any setuid/setgid/sticky display conventions rather than assuming a crate matches existing output. |
| [addts](../reference/ncdu-2.9.2/src/ui.zig#L545) (545) | Clamps to time_t, converts local time and timezone through libc, and shows invalid mtime fallback. This is display-only and single-threaded; retain presence/time-range distinction. |
| [hline](../reference/ncdu-2.9.2/src/ui.zig#L558) (558) | Wraps curses horizontal line rendering. Keep coordinate clipping and width conversions inside the adapter. |
| [Box.create](../reference/ncdu-2.9.2/src/ui.zig#L569) (569) | Centers and draws borders/title using curses acs_map with minimum-size guards and saturating positioning. Binding macro limitations explain the external symbol lookup; Rust can use terminal cells while preserving resize behavior. |
| [Box.tab](../reference/ncdu-2.9.2/src/ui.zig#L603) (603) | Draws numbered tab labels with selected header style. Reusable tabs keep info/help interactions consistent. |
| [Box.move](../reference/ncdu-2.9.2/src/ui.zig#L615) (615) | Translates box-relative coordinates into terminal coordinates. Explicit layout objects help beginner contributors add rows without global arithmetic. |
| [getch](../reference/ncdu-2.9.2/src/ui.zig#L622) (622) | Sets blocking mode, handles resize, retries blocking ERR up to 100 times with 10 ms sleeps, and fails on lost TTY. Bounded retry prevents infinite hangs; Rust event loop should distinguish timeout/resize/hangup/cancellation directly. |
| [waitInput](../reference/ncdu-2.9.2/src/ui.zig#L645) (645) | Waits for newline after child errors with separate Zig 0.14/0.15 APIs. Compatibility glue is unnecessary in Rust; choose the correct terminal input stream when stdin contains import data. |
| [runCmd](../reference/ncdu-2.9.2/src/ui.zig#L655) (655) | Suspends/restores curses, increments NCDU_LEVEL capped at nine, spawns with cwd/environment, and reports failures/nonzero status according to reporterr. Child commands inherit sensible terminal state; preserve raw paths/env ownership and avoid ad hoc shell escaping. |

### src/util.zig

Small shared integer, byte-buffer, sorting, home expansion, and line-reading utilities prevent policy drift between scanner/model/serialization/UI. LineReader exists solely to bridge Zig std.io changes between 0.14 and 0.15 (including delimiter API differences in 0.15.1/0.15.2). Rust should use one pinned toolchain and stable BufRead instead of copying that version scaffolding.

| Function (source line) | Purpose, reason, and Rust consequence |
| --- | --- |
| [castClamp](../reference/ncdu-2.9.2/src/util.zig#L8) (8) | Clamps any integer source to the destination range before casting. This safely maps negative OS sizes/timestamps and narrower counters; Rust helper names should make saturation explicit. |
| [castTruncate](../reference/ncdu-2.9.2/src/util.zig#L20) (20) | Bit-reinterprets signedness then narrows width without saturation. It is used for identifiers/bit fields, not usage totals; avoid confusing it with checked numeric conversions. |
| [blocksToSize](../reference/ncdu-2.9.2/src/util.zig#L28) (28) | Saturating-multiplies block count by 512. Centralization preserves sparse-file accounting and prevents wrap across UI/export; use an AllocatedBlocks conversion method. |
| [arrayListBufZ](../reference/ncdu-2.9.2/src/util.zig#L35) (35) | Temporarily appends NUL while returning a sentinel slice whose backing buffer may be invalidated by mutation. It bridges C APIs without duplicating text. Rust CString/CStr borrows must make the lifetime and interior-NUL policy explicit. |
| [fmt5dec](../reference/ncdu-2.9.2/src/util.zig#L45) (45) | Formats tenths into right-aligned five-byte decimal with stack arithmetic, avoiding floating formatter tables. Preserve numeric bounds/rounding while letting a small type enforce input range. |
| [strnatcmp](../reference/ncdu-2.9.2/src/util.zig#L74) (74) | Implements ASCII natural ordering: skip whitespace, compare zero-leading numbers left-aligned and others by digit length/value, then raw bytes. Numeric chunks need no integer parsing/overflow. Preserve pairwise comparator fixtures and retain the upstream attribution noted in source. |
| [expanduser](../reference/ncdu-2.9.2/src/util.zig#L176) (176) | Expands ~/ via HOME then passwd fallback and ~user via getpwnam, trims trailing home slashes, and preserves unresolvable input. Config-only use avoids surprising CLI expansion; put account lookup in Linux boundary and use reentrant APIs if it ever runs concurrently. |
| [LineReader(0.14).init](../reference/ncdu-2.9.2/src/util.zig#L210) (210) | Creates a 4096-byte buffered reader plus caller-provided fixed line stream. Buffering avoids per-byte file reads; Rust BufRead can preserve explicit line-size limits without version branching. |
| [LineReader(0.14).read](../reference/ncdu-2.9.2/src/util.zig#L217) (217) | Resets the line buffer, consumes through newline, returns final unterminated line, and distinguishes EOF from a blank line. Keep that distinction and oversized-line errors in config/exclusion parsing. |
| [LineReader(0.15).init](../reference/ncdu-2.9.2/src/util.zig#L229) (229) | Creates a streaming file reader over the caller's buffer using the newer API. This is toolchain adaptation only; no separate product feature is implied. |
| [LineReader(0.15).read](../reference/ncdu-2.9.2/src/util.zig#L233) (233) | Uses peek/toss delimiter operations, strips newline, and returns remaining EOF bytes, avoiding changed APIs implicated in the config hang fix. Rust tests should include empty/final/oversized lines, not emulate Zig's API selection. |

Ledger coverage: **300 declared functions across all 17 Zig files**. Duplicate names are distinguished by enclosing scope and source line.

## Existing test blocks: why they exist and what to retain

The snapshot contains 13 named Zig test blocks. They are useful regression seeds, not evidence of complete filesystem/concurrency coverage.

| Test | Purpose and Rust follow-up |
| --- | --- |
| [main.zig:23 imports](../reference/ncdu-2.9.2/src/main.zig#L23) | References every module so Zig discovers/compiles reachable module tests. Rust module declarations and cargo test replace this discovery mechanism; keep explicit integration coverage for all formats/modes. |
| [main.zig:663 argument parser](../reference/ncdu-2.9.2/src/main.zig#L663) | Exercises short clusters, attached values, long equals, positional/empty tokens, -- termination and exhaustion using nested opt/arg helpers. Preserve the token/consumption contract when adopting a parser crate. |
| [model.zig:507 entry](../reference/ncdu-2.9.2/src/model.zig#L507) | Allocates a plain entry, checks kind/optional-data flag/name, and frees it with testing allocator. Extend to all storage families, optional side data and lifetime/capacity behavior; the original only covers the simplest allocation. |
| [exclude.zig:81 parse](../reference/ncdu-2.9.2/src/exclude.zig#L81) | Covers empty/slash forms, literal/wild components, directory-only tails and continuation links. Preserve compiled grammar rather than pointer shape. |
| [exclude.zig:282 Matching](../reference/ncdu-2.9.2/src/exclude.zig#L282) | Covers absolute-root rules, unanchored rules, duplicate precedence and entered child states; testfoo factors repeated assertions. Add byte/escaping/locale fixtures before replacing fnmatch. |
| [util.zig:61 fmt5dec](../reference/ncdu-2.9.2/src/util.zig#L61) | Checks decimal tenths, zero-padding/alignment and maximum accepted formatting input without floats. Preserve exact strings and input bounds. |
| [util.zig:119 strnatcmp](../reference/ncdu-2.9.2/src/util.zig#L119) | Pairwise checks all ordered upstream natural-sort examples including zeros, dates, fractions and whitespace. This verifies more than adjacent comparisons; add comparator consistency/property coverage. |
| [ui.zig:177 shorten](../reference/ncdu-2.9.2/src/ui.zig#L177) | Checks narrow widths, wide Unicode glyphs, variation selectors and combining characters with locale setup. Existing odd combining output is documented in comments; deliberate improvements need updated expected behavior. |
| [ui.zig:450 fmtsize](../reference/ncdu-2.9.2/src/ui.zig#L450) | Checks SI/binary transitions, rounding around cutoffs, zero and u64 maximum. Keep fixtures so overflow/rounding changes cannot silently distort sizes. |
| [json_import.zig:304 JSON parser](../reference/ncdu-2.9.2/src/json_import.zig#L304) | Checks skipping and parsing null/bool/numbers, empty containers/strings and escapes. Source acknowledges missing malformed cases; add truncation, overflow, surrogate, non-UTF-8 and resource-bound cases. |
| [bin_reader.zig:251 CBOR int parsing](../reference/ncdu-2.9.2/src/bin_reader.zig#L251) | Checks immediate and every extended width, signed negative boundaries and unusually wide Zig integer types. Rust uses supported bounded types and explicit out-of-range rejection rather than imitating u1/i65 types. |
| [bin_reader.zig:276 CBOR string parsing](../reference/ncdu-2.9.2/src/bin_reader.zig#L276) | Checks empty/binary/text payloads and exact remaining cursor contents. Preserve raw bytes and cache-borrow lifetimes. |
| [bin_reader.zig:287 CBOR skip parsing](../reference/ncdu-2.9.2/src/bin_reader.zig#L287) | Checks nested definite/indefinite arrays/maps/byte chunks/tags and preservation of trailing garbage after one value. Add malformed/depth/cycle/work bounds instead of merely repeating valid examples. |

## Open baseline details to resolve during implementation

The documents are actionable without additional answers. Before making a final performance claim, establish the original command (including -t/-e/UI/export options), root path or a representative shareable fixture, storage/filesystem, cache regime, and whether 77 MB means GNU-time peak RSS or another measurement. Also establish the minimum supported Linux kernel and architecture for release packaging. Until these are known, retain the fstatat fallback, label synthetic results, and avoid declaring a specific speedup on the user's original tree.
