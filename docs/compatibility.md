# Live filesystem scope

The 2026-10-08 direction removes ncdu interoperability. rcdu now scans directories directly and calculates their sizes for the interactive browser or a plain-text `--quit-after-scan` result.

Removed: JSON, zstd JSON and EX1 imports/exports; lazy indexed browsing; compression and export block settings; saved-tree wire kind mappings; JSON scan reports; custom delete command integration; loading `/etc/ncdu.conf` and the ncdu user config; `NCDU_SHELL`, `NCDU_LEVEL` and `NCDU_DELETE_PATH` hooks. These flags now produce unknown-option errors.

Retained: parallel scanning, raw Linux names, sparse-file accounting, hardlink accounting, exclusions, ncurses navigation and sorting, refresh, built-in deletion, and optional shell access via `$SHELL`. Configuration uses rcdu paths only. Uppercase `D` and lowercase `d` start deletion of the selected file or directory. The default confirmation requires `y`; any other key cancels. `--no-confirm-delete` skips the dialog, while `-r` and `--disable-delete` block deletion.

Deletion walks the last observed tree, checks directory identities, and unlinks children before their parent. It does not follow directory symlinks or traverse child names created after the scan. A followed file symlink (`-L`) is unlinked as a symlink, leaving its target intact. During deletion, `q` stops further work. On an error, `a` aborts, `i` skips one error, and `I` skips subsequent errors. Rcdu rebuilds sizes for successful removals and reports when the selected entry remains. Use `r` to rescan external changes.

Historical source, licenses and benchmark results remain for provenance; none implement a runtime compatibility layer.
