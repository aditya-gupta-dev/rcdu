# rcdu: a fresh Linux disk usage browser

The implementation is being rebuilt following the user's 2026-10-08 direction: keep ncdu's curses interface and keys, but use a different scan/storage pipeline. This branch is work in progress. The first implementation is recoverable on [`archive/first-implementation-2026-10-08`](https://github.com/aditya-gupta-dev/rcdu/tree/archive/first-implementation-2026-10-08).

Read [the new design](docs/redesign.md) and [development progress](docs/progress.md). Original requirements and source analysis remain in [docs/INSTRUCTIONS.md](docs/INSTRUCTIONS.md) and [docs/GUIDE.md](docs/GUIDE.md); the latest user direction takes precedence where it changes the default worker count and terminal backend.

Every Cargo invocation uses `scripts/cargo` to restrict compilation/native compilation/test concurrency to half the process CPU budget. Runtime scanning defaults to all available CPUs and is controlled independently with `-t`.

The preserved ncdu 2.9.2 reference remains in `reference/ncdu-2.9.2/`. Historical measurements describe the archived implementation, not the new engine. No performance claim is made until this implementation is measured with equivalent results, including final accounting and browser costs.
