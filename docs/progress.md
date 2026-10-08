# Development progress

## 2026-10-08: restart requested

- Archived the first implementation and outstanding changes on `archive/first-implementation-2026-10-08` (checkpoint `f69403b`), pushed to GitHub.
- Removed the first implementation's Rust modules/examples/tests from main. Preserved the original documents/source/licenses, reproducible half-CPU build tools and historical measurements.
- Specified intra-directory metadata batching, compact per-worker storage, observation-time subtotals, directory-only ordinary reduction, automatic runtime workers and ncurses terminal ownership in `redesign.md`.
- Implemented the fresh batched scan library: bounded metadata queue, compact local directory-ID backlog, 24-byte entries with batch spans, observation-time ordinary subtotals and directory-only final ordinary reduction. Four new behavior tests pass for wide-directory worker participation, raw names/sparse files, sibling/outside hardlinks, repeated broad/deep scans, symlinks, cancellation and panic join. Formatting and all-target clippy pass with six build jobs.
- The executable/session, exclusions/config, curses, codecs, mutations and new performance measurements are still in progress. The binary currently reports that construction is in progress; it does not silently present a partial scanner as the finished application. Earlier full-application results apply to the archived version only.
