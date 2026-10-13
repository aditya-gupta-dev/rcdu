# Credits and licensing

rcdu's Rust implementation is MIT licensed. The repository's [LICENSE](../LICENSE) retains Yorhel's copyright notice for ncdu-derived work. The ncdu Zig source and manual are no longer included in this repository.

The Rust natural comparator is an altered implementation following ncdu's port of Martin Pool's strnatcmp, not the original C implementation. Copyright (C) 2000, 2004 Martin Pool. The full upstream notice is retained in `LICENSES/NaturalSort.txt`; source: https://github.com/sourcefrog/natsort/.

Dependency notices apply to libc, unicode-width and unicode-segmentation. `cargo tree --locked` shows the resolved graph. The Linux terminal adapter links the system ncursesw library and owns terminal initialization, suspension, restoration and cleanup. Zstandard is no longer a dependency after removal of saved-tree formats.
