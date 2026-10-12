# rcdu

A Linux disk usage scanner and interactive ncurses browser. It scans the live filesystem, calculates allocated and apparent sizes, and lets you navigate, refresh and delete files or directories. Scanning uses available CPUs by default; `-t 1` selects a single worker.

```sh
scripts/cargo build --release
target/release/rcdu /path/to/directory
target/release/rcdu --quit-after-scan /path/to/directory
```

Use arrows or `hjkl` to navigate, `D` (or `d`) to delete the selected file or directory, `r` to rescan the current directory, and `?` for help. By default, `D` opens a confirmation dialog: press `y` to delete or any other key, including Enter, to cancel. `--no-confirm-delete` makes `D` start immediately; `-r` or `--disable-delete` prevents deletion.

During a directory deletion, rcdu shows progress and `q` stops the remaining work. It deletes the entries found by the last scan, without following directory symlinks. A new child name created after that scan is not traversed and can keep a directory from being removed. Errors offer `a` to abort, `i` to ignore one, or `I` to ignore all; rcdu reports an incomplete deletion afterward. Successful removals update the displayed sizes. Press `r` to scan again when the filesystem has changed outside rcdu.

The app has no saved-tree import/export, JSON/zstd/EX1 formats, ncdu configuration loading, or ncdu environment hooks. Its optional configuration lives at `/etc/rcdu.conf` and `$XDG_CONFIG_HOME/rcdu/config` (default `~/.config/rcdu/config`); command-line arguments override it. `--ignore-config` skips configuration files. `--quit-after-scan` prints plain-text root sizes and exits.

[Editable Excalidraw JSON](docs/diagrams/rcdu-architecture.excalidraw.json) and [SVG preview](docs/diagrams/rcdu-architecture.svg) describe the current code. The [diagram before cleanup](docs/diagrams/rcdu-before-cleanup.excalidraw.json) preserves the original architecture requested before removing compatibility code. Open either JSON file in Excalidraw using **Open**.

```sh
scripts/cargo test
scripts/cargo build
scripts/integration
scripts/cargo clippy --all-targets -- -D warnings
```

Every Cargo invocation uses `scripts/cargo` to restrict build/test concurrency to half the available CPU budget; scan concurrency is controlled independently. See [the scan design](docs/redesign.md) and [measurement notes](docs/measurement.md). Earlier compatibility documents, reference source and benchmark results are historical; they do not describe the current feature set. Required source and license notices remain in [credits](docs/credits.md).
