# New development direction (2026-10-08)

The user requested a fresh implementation rather than a structural port: ncdu keys and curses layout, automatic use of available CPUs, efficient memory and measured speed, with progress committed to Git. This supersedes the first implementation's one-worker default and ANSI terminal adapter. Existing work was checkpointed and pushed to an archive branch before removing its Rust source, examples and tests from main. Original requirements, source/licenses, build budget and measurement tools remain references.

## Pipeline

```text
directory enumeration (buffered getdents64)
       |
       +--> bounded metadata batches --> any available scan worker
       |                                own compact arena + ordinary subtotal
       +--> independent child directories --> shared work queue / local ID backlog
                                              |
                            joined worker arenas, no per-file merge/copy
                                              |
                    directory-only ordinary reduction + inode/ancestor reduction
                                              |
                         ncurses browser / interoperable exports
```

The previous directory-level scheduler could leave most workers idle on a flat, wide directory. The new scheduler exposes metadata batches within that directory as independent work. Names are copied into bounded batch buffers, not full paths; batches share an owned parent descriptor. The queue never blocks publishers. Full-queue metadata work is processed locally, and discovered directory IDs can be deferred locally without retaining directory descriptors or recursively growing the call stack. The compact local ID backlog is proportional to discovered directories, not file bytes or open handles. Descriptor capacity limits the queue and requested pool size.

Each scanning worker appends compact records and raw names into its own arena and accumulates ordinary sizes at observation time. A coarse span records where a batch's entries live; directory listings use spans rather than scanning every file or storing per-file next pointers. Parent directory IDs are dense. Final ordinary accounting visits directories, not every ordinary file again. Hard-link observations remain separate and are reduced by identity across containing ancestors, preserving sibling/shared usage. A directory's own size is included. Overflow saturates.

Default `-t 0` selects available CPU parallelism; `-t 1` remains available for slow/random-I/O storage and comparisons. One terminal owner calls ncurses. Extra workers can improve metadata throughput but cannot remove filesystem/storage bottlenecks or guarantee all CPU cores stay busy. GPU/CUDA does not provide the Linux directory enumeration/stat interface; adding transfers or hashing contents would not accelerate this workload. All-CPU utilization is a scheduling goal, not a fabricated benchmark result.

The design deliberately differs from the old pointer/ring tree and the archived whole-model ordinary postorder walk. Behavioral interoperability still requires the same size definitions and wire constants; those semantics are not an algorithm to replace with approximations.

## Optimization policy

Start with ordinary optimized release builds and retain an optimized profiling build. Compare ThinLTO/single-codegen-unit distribution builds, CPU-native builds and any allocator/PGO experiments separately, using the same correctness oracle and at least five alternating runs. Keep unwind/terminal restoration. Do not assume that stacking flags improves metadata throughput. Preserve all runs; investigate >5% time/RSS regressions. Old results are useful baselines only, not evidence for this engine.

Both elapsed time and peak RSS, as well as wide-directory CPU participation, must be measured. The original 1,394,617-file/~60-GB workload and exact command remain unavailable; synthetic results cannot certify that target. Full compatibility includes config, exclusions, curses/key behavior, refresh, safe deletion/shell, JSON/zstd and EX1/lazy browsing. Until verified, these remain explicit development gates rather than implied completed features.
