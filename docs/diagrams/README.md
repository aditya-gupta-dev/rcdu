# Architecture diagrams

Open either `.excalidraw.json` file with Excalidraw's **Open** command. The scene uses native editable rectangles, text and arrows; it contains no embedded screenshot.

- [Current architecture JSON](rcdu-architecture.excalidraw.json) / [SVG preview](rcdu-architecture.svg): live scanning, worker scheduling, storage/accounting, browser, deletion and refresh.
- [Before-cleanup JSON](rcdu-before-cleanup.excalidraw.json) / [SVG preview](rcdu-before-cleanup.svg): the working tree reviewed before removing JSON/EX1 compatibility. This is a historical snapshot, including source changes already present at the start of the review.

The scanner queues both directory IDs and metadata batches. Workers append their own records and names; directory spans identify each batch's children. Joining workers transfers their Parts into the Model. Final accounting reduces ordinary directory totals and groups hardlinks by device/inode across ancestors.

The session chooses interactive browsing or a plain-text root-size result. `D`/`d` starts deletion; default confirmation requires `y`, and read-only configuration blocks it. The browser shows progress, accepts `q` to stop, and offers abort/ignore choices on errors. The walker removes observed entries through verified directory descriptors and `unlinkat`, without following symlinks or traversing child names created after scanning. Mutation rebuilding removes successfully deleted entries, adjusts hardlinks and recalculates totals. An incomplete deletion is reported; refresh replaces the current subtree with a new scan.

Historical reference source, licensing and older benchmark results remain outside the runtime path.
