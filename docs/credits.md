# Credits and licensing

rcdu's Rust implementation is MIT licensed. The ncdu 2.9.2 source, manual and original MIT notice by Yorhel are preserved in `reference/ncdu-2.9.2/` and `LICENSE`.

The Rust natural comparator is an altered implementation following ncdu's port of Martin Pool's strnatcmp, not the original C implementation. Copyright (C) 2000, 2004 Martin Pool. The full upstream notice is retained in `LICENSES/NaturalSort.txt`; source: https://github.com/sourcefrog/natsort/.

Dependency notices remain applicable: libc, zstd/zstd-safe/zstd-sys (bundled Zstandard 1.5.7), unicode-width and unicode-segmentation, plus Cargo.lock's native build dependencies. `cargo tree --locked` shows the exact resolved graph. No ncurses library is linked by Rust; the Linux terminal adapter owns `/dev/tty`, termios and ANSI rendering.
