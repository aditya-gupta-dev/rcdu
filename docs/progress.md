# Development progress

## 2026-10-08: restart requested

- Archived the first implementation and outstanding changes on `archive/first-implementation-2026-10-08` (checkpoint `f69403b`), pushed to GitHub.
- Removed the first implementation's Rust modules/examples/tests from main. Preserved the original documents/source/licenses, reproducible half-CPU build tools and historical measurements.
- Specified intra-directory metadata batching, compact per-worker storage, observation-time subtotals, directory-only ordinary reduction, automatic runtime workers and ncurses terminal ownership in `redesign.md`.
- Implemented the fresh batched scan library: bounded metadata queue, compact local directory-ID backlog, 24-byte entries with batch spans, observation-time ordinary subtotals and directory-only final ordinary reduction. Four new behavior tests pass for wide-directory worker participation, raw names/sparse files, sibling/outside hardlinks, repeated broad/deep scans, symlinks, cancellation and panic join. Formatting and all-target clippy pass with six build jobs.
- Added the fresh CLI/config parser, exclusions/cache/kernel rules, ncursesw owner/cleanup adapter, session progress and curses browser navigation/sorts/toggles/help/details. Seven Rust behavior tests pass; the owned-PTY check verifies help, format tab, details, resize, normal quit and SIGTERM with exact terminal-attribute restoration. Formatting/all-target clippy pass. `--scan-report FILE` records final totals and per-worker record counts for scheduling diagnostics.
- The user identified `/home/adi` as the original benchmark root. The original command/options remain unknown. Generated fixtures inside that root will be explicitly handled in comparisons, and historical file counts must be remeasured rather than assumed.
- Codecs, lazy browsing, refresh/shell/deletion, fuller differential UI coverage and new measurements are still in progress. Unsupported actions currently report that they are being implemented. Earlier full-application results apply to the archived version only; this restart is not yet the completed replacement.
