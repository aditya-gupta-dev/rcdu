# Command line and configuration

`rcdu [options] [directory]` scans the directory (default `.`). `-f FILE` imports instead; scan root and import are mutually exclusive. Give only one import/output selection. Option parsing accepts short clusters, attached values (`-t4`), long equals values (`--threads=4`) and `--` before a path beginning with `-`. Paths and patterns remain bytes.

| Options | Behavior |
| --- | --- |
| `-h`, `-?`, `--help`; `-v`, `-V`, `--version` | Help/version |
| `-t`, `--threads N` | 1 default; 0 automatic; 1–255 explicit workers |
| `-x`, `--one-file-system`; `--cross-file-system` | Same device / cross devices; applies to followed file targets too |
| `-e`, `--extended`; `--no-extended` | Store uid/gid/mode/mtime presence |
| `-L`, `--follow-symlinks`; `--no-follow-symlinks` | Follow file targets, never directory symlinks |
| `--exclude PATTERN`; `-X`, `--exclude-from FILE` | Component fnmatch; leading `/` anchors to filesystem root, trailing `/` matches directories only |
| `--exclude-caches`, `--include-caches` | Exact CACHEDIR.TAG signature filter |
| `--exclude-kernfs`, `--include-kernfs` | Filter kernel filesystem child crossings; selected root still scanned |
| `-0`, `-1`, `-2` | No / line / full scan UI; export and quit-after-scan default to no UI |
| `-q`, `--slow-ui-updates`; `--fast-ui-updates` | 2 seconds / 100 ms redraw intervals |
| `-f FILE` | Raw or Zstandard JSON; seekable EX1 binary; JSON stdin `-` |
| `-o FILE`; `-c`, `--compress`, `--no-compress` | JSON output, optionally Zstandard; stdout `-` |
| `-O FILE` | EX1 binary output, always Zstandard |
| `--compress-level N`; `--export-block-size N` | Levels 1–20; binary block target 4–16000 KiB, default 64 KiB |
| `-r`, `-rr` | Disable delete; second occurrence also disables shell |
| `--enable-shell`, `--disable-shell` | Shell capability |
| `--enable-delete`, `--disable-delete` | Deletion capability |
| `--enable-refresh`, `--disable-refresh` | Refresh capability |
| `--delete-command COMMAND` | `/bin/sh -c` command with quoted target appended and `NCDU_DELETE_PATH` set |
| `--confirm-quit`, `--no-confirm-quit` | Quit prompt; off by default |
| `--confirm-delete`, `--no-confirm-delete` | Default-No delete prompt; on by default |
| `--sort FIELD[-asc\|-desc]` | name, disk-usage, apparent-size, itemcount, mtime |
| `--apparent-size`, `--disk-usage`; `--si`, `--no-si` | Size column and decimal/binary units |
| `--shared-column off\|shared\|unique` | Hardlink usage column |
| `--show-hidden`, `--hide-hidden` | Dot names, tilde suffixes and excluded entries; display only |
| `--show-itemcount`, `--hide-itemcount`; `--show-mtime`, `--hide-mtime` | Extra columns; mtime needs extended data |
| `--show-graph`, `--hide-graph`; `--show-percent`, `--hide-percent` | Usage columns |
| `--graph-style hash\|half-block\|eighth-block` | Bars; legacy `eigth-block` spelling also accepted |
| `--group-directories-first`, `--no-group-directories-first` | Directory grouping |
| `--enable-natsort`, `--disable-natsort` | Natural/raw byte name order |
| `--color off\|dark\|dark-bg` | Color theme; off by default |
| `--ignore-config`; `--quit-after-scan` | Bypass config; construct/finalize complete model then exit |
| `--metadata-backend fstatat\|statx` | Rust measurement option; fstatat default, optional statx with fallback |

Configuration loads `/etc/ncdu.conf`, then `$XDG_CONFIG_HOME/ncdu/config` or `$HOME/.config/ncdu/config`, then CLI. Scalar settings override; exclusions accumulate. `--ignore-config` is recognized before opening any config. Lines accept whitespace or `=` between an option and value, blank lines and leading `#` comments. This is not shell quoting. Prefix `@` suppresses invalid/unknown option errors without partially applying that line. Lines are limited to 4096 bytes; final unterminated lines are accepted. Config exclusion patterns/files expand `~` and `~user` through account lookup. CLI patterns are literal.

Browser keys: arrows/`hjkl`, Home/End/Page Up/Page Down, Enter to enter, Backspace/`<` to parent, `q` quit, `?` help. `n` sorts names, `s` sizes, `C` item counts, `M` extended mtime; repeating a sort reverses it. `a` changes apparent/allocated, `e` hidden entries, `t` directories first, `c` counts, `m` mtime, `g` cycles graph/percent, `u` cycles shared/unique. `i` opens details, `1`/`2` choose details/hardlinks, `j`/`k` change selection in details, Enter jumps to a known hardlink, `i`/`q`/Escape close. `r` refreshes current directory, `d` deletes selected entry, `b` launches `$NCDU_SHELL`, `$SHELL`, or `/bin/sh` in the current directory with incremented `NCDU_LEVEL` (maximum 9).

Imports disable shell/delete/refresh by default. Explicit capability options can enable actions for JSON imports with a safe absolute root. Indexed browsing always disables delete/refresh and hardlink navigation. Deletion operates only on scanned children; a newly appeared unscanned child prevents removing its directory. Errors offer Abort, Ignore or Ignore all. Confirmed mutations are followed by observation refresh; refresh failure retains the existing model and reports the failure.

Single-worker JSON streams with bounded depth state. Multiple-worker JSON stages observations in memory without computing unused totals. Binary scans stream in parallel with a bounded block per worker and per-directory completion state; link-heavy trees additionally retain inode maps. JSON exports containing non-UTF-8 names use legacy raw bytes and may not be strict UTF-8 JSON. A late directory enumeration error after the first child cannot be added to the already-written JSON object; binary/memory output records it.
