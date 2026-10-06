# Compatibility gates

Status is evidence, not an intention. A feature is verified only for listed fixtures.

| Feature | Status | Evidence / remaining gate |
| --- | --- | --- |
| Preserved Zig 2.9.2 reference/license/manual | Verified | `reference/ncdu-2.9.2` |
| Half-core reproducible build wrapper | Implemented | affinity and visible ancestor cgroup quota discovery |
| CLI/config precedence and all legacy flags | Pending | source `main.zig` |
| Byte names, ordinary/sparse/directory own sizes | Pending | Linux fixture integration |
| Descriptor-relative scan, statx fallback | Pending | syscall equivalence fixtures |
| Fixed bounded scan pool/cancellation | Pending | scheduling/FD stress |
| Hardlinks total/shared at every ancestor | Pending | sibling/external/refresh fixtures |
| Exclusions/cache/kernel/symlink semantics | Pending | differential fixtures |
| Raw/compressed streaming JSON both directions | Pending | Zig cross-read |
| Block binary export/indexed eight-block reader | Pending | Zig cross-read/corruption |
| Low-memory indexed browser | Pending | first frame and listing RSS |
| Terminal navigation/sorts/details/help/config | Pending | PTY tests |
| Refresh and shell | Pending | temporary tree/PTY |
| Built-in/custom deletion/abort/error handling | Pending | owned fixtures only |
| Debug/release tests, fmt, clippy | Pending | final checks |
| Both time and RSS below equivalent Zig baseline | Pending | original workload unavailable |
