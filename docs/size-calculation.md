# How rcdu calculates sizes

![Filesystem observations, scan workers, directory reduction, hard-link example and export paths](size-calculation.png)

[Open the full-resolution PNG](size-calculation.png) · [Editable SVG](size-calculation.svg) · [Example input](diagrams/size-example.json)

This describes the current Rust implementation, including the differences between an interactive scan, JSON export and binary export. The diagram is rendered from exact text and shapes so formulas remain readable. It describes code behavior, not a benchmark or a claim about filesystem-wide physical space recovery.

## 1. There are two independent sizes

| Value | Kernel observation | Unit | Meaning |
| --- | --- | --- | --- |
| Apparent size | `st_size` / `stx_size` | bytes | The object's logical length. For a regular file this includes holes. For a directory this is the directory's own reported length. |
| Allocated size | `st_blocks` / `stx_blocks`, multiplied by **512** | bytes | Allocation reported by the filesystem for this object. The multiplier is always 512, even when the filesystem block size is 4096. |
| Identity | device + inode | pair of integers | Hard links share this pair. An inode number alone is not sufficient across filesystems. |
| Link count | `st_nlink` / `stx_nlink` | directory entries | Used to determine whether another link exists outside a particular subtree. |

For example, a file can have `st_size = 1000` and `st_blocks = 8`: its apparent size is 1000 B and its allocated size is `8 × 512 = 4096 B`. A sparse 1 GiB file with no allocated data can report `st_size = 1073741824`, `st_blocks = 0`. An empty regular file can report zero in both columns. These observations are examples, not guaranteed allocation layouts on every filesystem.

rcdu reads directory entries and metadata, rather than reading every file's contents or counting bytes in a stream. Directory enumeration uses `getdents64` with an 8192-byte buffer and skips `.` and `..`. The default metadata backend is `fstatat`; `--metadata-backend statx` selects an optional backend. `statx` retries through `fstatat` when unavailable, rejected with ENOSYS/EINVAL/EPERM, or missing requested basic fields. Other metadata errors remain errors. Cache exclusion is a bounded content-reading exception: it reads the cache marker signature.

The `fstatat` path clamps negative sizes/block counts to zero. Observation link counts are capped at `0x7fffffff`. In the memory model, block counts occupy 60 bits and are capped at `(1 << 60) - 1`. Allocation multiplication and byte/item sums saturate at `u64::MAX`; they do not wrap into small numbers. Streaming collectors multiply the observed block count directly with saturation. On ordinary Linux observations these paths agree; contrived oversized values at the library boundary can encounter the memory-model block cap first.

Sources: [Linux metadata and enumeration](../src/os/linux.rs), [entry representation and totals](../src/model.rs).

## 2. Yes, scanning supports multiple threads

```sh
# Default configuration: one scanning worker.
target/release/rcdu --ignore-config /path/to/tree

# Four scanning workers.
target/release/rcdu --ignore-config -t 4 /path/to/tree

# Automatically choose the parallelism available to the process.
target/release/rcdu --ignore-config -t 0 /path/to/tree
```

`-t` and `--threads` are equivalent. The default is **1**, automatic mode uses Rust's `available_parallelism()` with a fallback of one, and the supported worker limit is 255. Config files can change the default; the commands above ignore them. The descriptor limit can reject a requested count before scanning starts. Automatic selection is not a guarantee that the selected count is fastest for the storage device.

For an ordinary browser scan, the calling thread opens the root and starts exactly N scoped worker threads named `scan-0`, `scan-1`, and so on. It owns progress/terminal operations while workers scan. These are OS threads, not one thread per file or an async task per file. Each worker has a requested 256 KiB stack.

Each directory has **one enumerator**. It observes that directory's immediate children in order; discovered child directories can be handed to other workers. Independent directory branches can run simultaneously. A single directory containing a million files is still enumerated by one worker, so extra workers may be idle until there are subdirectories to process. A single long chain also exposes little independent work. Parallel scanning does not imply N-fold speedup: storage latency, filesystem locks, allocation, scheduling and the final accounting pass all matter.

The shared LIFO queue contains at most 16 directory tasks, reduced when the descriptor budget requires it. Before publishing a task, the worker increments an **outstanding** counter. A completed directory decrements it once. An empty queue is not sufficient to terminate: other workers may still discover children. Workers wait on a condition variable until work arrives or outstanding reaches zero.

If the queue is full, the publishing worker processes the child locally using an explicit depth-first frame stack. It does not block waiting for another publisher to free space. Up to 16 ancestor descriptors/buffers per worker are retained, again reduced by the descriptor budget; deeper frames suspend their cursor, release resources and reopen relative to the root when resumed. Opened/reopened directory identities are verified before their children are combined with prior observations.

Memory-mode workers own separate `Part` arenas: 24-byte entry records, raw byte names in a contiguous NUL-terminated arena, compressed runs of parent IDs, and directory/hard-link/extended-metadata side tables. There is no mutex on every ordinary file entry. Entry IDs encode worker in the high eight bits and slot in the low 24 bits. Progress updates are batched every 256 entries. Workers return coarse directory completion records rather than sending a message for every file.

**The memory-mode size accounting runs on the calling thread after every worker has joined.** Neither the ordinary postorder pass nor the hard-link ancestor reduction is currently parallel. Thus the application supports multithreaded scanning, with a serial final reduction and one terminal owner. Fatal worker/setup/output failures and cancellation wake sleepers and join started workers before returning; cancelled partial memory models are discarded.

Compilation parallelism is a separate setting: [`scripts/cargo`](../scripts/cargo) limits builds/native builds/tests to half the process CPU budget. It does not select the scanner's `-t` value.

Sources: [scan pool, handoff and join](../src/scan.rs), [descriptor budget](../src/os/linux.rs), [worker storage](../src/model.rs).

## 3. Ordinary directory accounting

After joining the workers, rcdu attaches each completion record's child-list head and read-error flag to the directory. It traverses directories in **postorder**, so a child directory's ordinary totals exist before its parent's totals are computed.

For allocated bytes and apparent bytes independently:

```text
ordinary(D) = own size of directory D
            + sizes of immediate non-hard-link leaves
            + ordinary totals of immediate child directories

items(D) = number of immediate child entries
         + sum of items(child directory)
```

A directory's own metadata size is included exactly once. Its `items` count excludes itself and includes all observed descendant entries. Every hard-link name counts as an item, even though its bytes are deduplicated later. Excluded/error placeholders are observed items with zero bytes. Entries inside an excluded or unreadable subtree that were never observed do not appear in the count.

Hard-link leaf bytes are deliberately omitted from the ordinary pass. While traversing reachable children, rcdu collects them into groups keyed by `(device, inode)`. This prevents the parent from accidentally inheriting one duplicate contribution from every child directory.

The pass also propagates read-error/descendant-error flags. With extended metadata, it computes the newest observed mtime over the directory and its descendants; mtime has no role in calculating sizes. Detached entries are unreachable and contribute nothing.

Source: [`Model::recount_with_cancel`](../src/model.rs).

## 4. Hard links: one contribution per containing ancestor

For every hard-link identity H, rcdu walks upward from the parent of **each observed name**. It builds `count(D, H)`, the number of names for H inside each ancestor D. It then adds one representative object's allocated/apparent size to each ancestor in that map, regardless of whether the count is one, two or a thousand.

```text
total(D) = ordinary(D)
         + sum(size(H) for distinct hard-link identities present below D)

effective_nlink(H) = consistent nonzero reported nlink
                  OR number of names observed for H in the whole model
                     if nlink is zero or inconsistent

shared(D) = sum(size(H) when count(D, H) < effective_nlink(H))
unique(D) = saturating_sub(total(D), shared(D))
```

The fallback is useful for imported data that marks a hard link but omits `nlink`. For a stable live inode, all links normally report the same sizes/counts. If a concurrent change or inconsistent import provides different sizes, the memory reducer uses the first record in its collected group as its representative, rather than reconciling sizes or choosing the latest timestamp. Traversal/worker ordering is not a snapshot guarantee.

“Shared” means this hard-linked object has another name outside this particular directory subtree. It does **not** mean multiple names exist somewhere inside the subtree. Two names entirely inside D still contribute once to D and are unique to D when all reported links are inside D. “Unique” is a hard-link accounting classification; it is not a prediction of exact space reclaimed by deletion.

A single process-wide “already counted this inode” set would be wrong: if links exist in sibling directories a and b, both a and b must show the inode's size. Their parent must show it once. Likewise, summing their **finished** directory totals would double-count it. The reducer adds hard-link contributions to ancestors independently after completing the ordinary pass.

Hard-linked leaf rows show their own size. Their individual `Totals` use zero shared fields; the shared/unique directory columns describe the containing subtree's deduplicated totals. Device identity prevents unrelated equal inode numbers on different filesystems from merging. The Linux classifier treats any non-directory object with `nlink > 1` as hard-link-accounted, including hard-linked nonregular objects.

### Worked example in the PNG

Assume `/root`, `/root/a`, and `/root/b` each have own allocated **and** apparent size 4096 B. These directory sizes are explicit illustrative metadata, not filesystem assumptions.

```text
/root/
  a/
    plain     apparent 1000 B; allocated 4096 B
    h1 ─┐
    h2 ─┼── inode H: apparent 6000 B; allocated 8192 B
  b/    │           device 7; inode 42; nlink 4
    h3 ─┤
    sparse    apparent 1073741824 B; allocated 0 B
/outside/h4 ┘       fourth link, outside the scanned root
```

| Directory | Observed H names | Allocated B | Apparent B | Shared allocated B | Shared apparent B | Unique allocated B | Items |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| a | 2 / 4 | 16384 | 11096 | 8192 | 6000 | 8192 | 3 |
| b | 1 / 4 | 12288 | 1073751920 | 8192 | 6000 | 4096 | 2 |
| /root | 3 / 4 | 24576 | 1073761112 | 8192 | 6000 | 16384 | 7 |

The allocated root result is `3 × 4096` for the directories, `4096` for plain, `0` for sparse, and **one** `8192` for H: **24576 B**. Adding `a + b + root's own size` would give 32768 B and overcount H by 8192 B. The root apparent result is `3 × 4096 + 1000 + 1073741824 + 6000 = 1073761112 B`.

If the outside link is removed and all H records report `nlink = 3`, the root's total bytes and items stay the same, but its shared values become zero. a and b still mark H shared because neither contains all three links. Excluded links can similarly make an observed inode appear shared when its other names are outside the accounted tree.

The [example JSON](diagrams/size-example.json) encodes these exact observations. Verify against the Rust reducer, without assuming a particular filesystem's directory allocation:

```sh
scripts/cargo build --locked --release --example normalize
target/release/examples/normalize < docs/diagrams/size-example.json
```

Output columns are path in hex, kind, own allocated, own apparent, total allocated, total apparent, shared allocated, shared apparent, items, link count, direct error, descendant error, identity and extended metadata. The root path column is empty. Existing [scan tests](../tests/scan.rs) check links in siblings/outside the root at 1/2/4 workers; [format tests](../tests/formats.rs) check unknown counts, saturation and byte-preserving imports.

## 5. What gets counted or skipped

| Situation | Current behavior |
| --- | --- |
| Default symlink handling | Observe the link itself without following. Its apparent size is normally the target-path length, and allocation is the link's reported block count. |
| `-L` | Follow a symlink to a nondirectory target when target metadata succeeds. Never recurse into a directory symlink. On failure retain the link observation. A followed hard-link target on a different device is treated as ordinary to avoid cross-device link grouping. |
| `-x` | A child on another device becomes a zero-byte `OtherFs` placeholder; do not descend. This includes followed targets after the `-L` step. |
| Pattern/cache/kernel exclusions | Zero-byte placeholder; do not descend. Directory-only patterns are resolved after type observation. Cache exclusion checks the marker; kernel exclusion checks a filesystem boundary. The explicitly opened root is not processed as a child exclusion. |
| Child metadata fails | Zero-byte `Error` entry and error indication. The failure is not interpreted as a genuinely zero-size file. |
| Directory opens/stats but cannot be enumerated completely | Keep its observed own size and any known children; mark the read error. Unseen children have unknown usage. Failure to open/stat the scan root fails the scan itself. |
| Hide hidden/excluded rows in the browser | Change visibility, not underlying totals. A scan exclusion changes accounting; a display filter does not. |
| Reflinks, deduplicated/compressed storage, snapshots | No cross-inode extent-sharing analysis. rcdu reports filesystem metadata; hard-link deduplication only merges equal device/inode identities. |
| Concurrent filesystem changes | Best-effort observations with identity checks, not a point-in-time filesystem snapshot. |

Apparent and allocated sizes can legitimately differ substantially. Totals also need not match `df`: filesystem-wide free/used space includes objects and overhead outside this scanned namespace, and the scanner does not track unlinked-but-open files or globally shared extents. A link count larger than the observed count identifies sharing without requiring rcdu to find the outside names.

## 6. Export/import paths have different accounting lifecycles

| Mode | Traversal/threads | Accounting |
| --- | --- | --- |
| Live browser / `--quit-after-scan` | N scan workers; retain model | Join workers, then serial ordinary + global hard-link ancestor passes. |
| JSON or zstd JSON export with **explicit workers = 1** | Serial depth-first scanner on calling thread | Stream own observations; no complete model or directory-total reduction. Reader reconstructs totals. |
| JSON or zstd JSON export with workers > 1, or automatic setting 0 | Fixed scan pool; retain observations temporarily | Stage the model without recount; JSON writer emits own observations, then discards staging. Setting 0 chooses this path even if it resolves to one actual worker. |
| Live EX1 binary export (`-O`) | Fixed scan pool; per-worker encoding and zstd block compression | Incremental per-directory ordinary totals + hard-link maps; finalize when enumeration and all child tokens complete. Store cumulative/shared totals for lazy browsing. |
| JSON import | Serial parser; retained model | Build hierarchy and recount; zstd is decompression around the same parser. `dsize` is bytes and is converted to blocks by `dsize >> 9`, truncating a nonmultiple of 512. |
| Indexed binary browsing | Serial current-listing loads, up to eight cached decompressed blocks | Use stored cumulative directory totals; do not scan the filesystem or recount a partial listing. Cache has a combined 64 MiB budget. |
| Full binary import for conversion | Serial reader; retained model | Load all observations, then recount rather than trusting stored directory totals. |

Binary streaming uses `pending = 1` for a directory's enumeration token and adds one token per child directory. A worker drops the enumeration token when enumeration ends. Child completion merges ordinary totals, observed link counts and error flags into the parent, then drops that child's token. Exactly the worker that makes pending reach zero finalizes the directory. Parent finalization can then cascade upward iteratively. This prevents a directory from being written before a slower child finishes.

Crucially, child **ordinary** totals and inode maps are merged, rather than the child's already deduplicated full total. Child-directory own sizes are accounted at discovery; finalization adds the current directory's own size to the stored record. `LinkCount` stores one representative size and observed count per identity. Worker compression happens outside the physical output mutex. Directory mutexes protect sibling references/completion state; the output mutex serializes block writes and index publication. There is no nested full-size compression worker pool.

An edge-case distinction: the streaming binary sink's missing/inconsistent-`nlink` fallback uses the **current subtree's** observed count, while the in-memory reducer uses the **whole model's** count. Therefore these fallback cases can classify shared bytes differently in child directories. Normal stable live hard links have consistent positive `nlink`; imported JSON converted through the model writer uses the global recount result. This is an actual current implementation distinction, not a claim that every malformed/racing observation yields identical mode results.

Sources: [session routing](../src/session.rs), [JSON parser/writer](../src/format/json.rs), [streaming link reduction](../src/sink.rs), [parallel binary completion](../src/format/binary_pool.rs), [EX1 reader/writer](../src/format/binary.rs).

## 7. Display, refresh and deletion

The main size column selects allocated bytes by default or apparent bytes with `--apparent-size` / browser key `a`. Shared and unique select the matching byte metric. Formatting uses binary units by default (KiB/MiB/GiB) or decimal units with `--si`; this only changes labels/rounding, not the totals. Formatting rounds to one decimal place using wide intermediate arithmetic.

The percent column uses `floor(row_bytes × 1000 / current_directory_bytes)` and renders tenths of a percent; zero parent usage yields zero. The graph uses `floor(row_bytes × 80 / largest_child_bytes)`, rendered as ten cells with up to eighth-cell precision according to style. Its scale is the largest child, not the current directory total. Because hard-link-containing siblings overlap, visible row totals/percentages need not add up to the parent's total/100%; because directories own bytes and display filters can hide entries, a simple row sum can also undercount.

Refreshing scans the current subtree with the same configured pool, constructs a replacement, rebuilds the reachable whole model and recounts all ancestor/identity contributions. A root setup failure retains prior data. This avoids retaining abandoned arena generations but temporarily requires old + replacement + compacted model memory.

Deletion unlinks known entries through verified no-follow directory handles. Successful removals detach observations, adjust known surviving link counts and recount; partial failure retains remaining observations. The browser then attempts a refresh to obtain surviving metadata. Merely subtracting the deleted row's bytes from its ancestors would be wrong when another hard link survives. If an external link remains, deletion may remove a name without freeing the inode's allocation. Filesystem extent sharing, open handles and snapshots further separate the displayed unique column from actual reclaimed space.

Sources: [browser rendering and refresh](../src/browser.rs), [mutations and recount](../src/delete.rs), [unit formatting](../src/ui/display.rs).

## Re-render the diagram

```sh
python3 docs/diagrams/render-size-calculation.py
```

The generator uses Python's standard library and the `rsvg-convert` executable (librsvg). It writes `docs/size-calculation.svg` and `docs/size-calculation.png`. It does not build Rust or require an image-generation service. The PNG is 2600 × 3640 pixels; zoom in to read individual panels.
