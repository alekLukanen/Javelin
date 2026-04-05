# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
# Build
cargo build
cargo build --release

# Run all tests
cargo test

# Run a single test with output
cargo test <test_name> -- --nocapture

# Run the example binary
cargo run --bin main

# Profile with flamegraph
cargo flamegraph --root --bin main
```

## Architecture

Javelin is an LSM-tree (Log-Structured Merge Tree) embedded key-value storage engine. The public API (`DB`) in `src/core/db.rs` exposes `get`, `set`, `delete`, and `iterate`.

### Write Path

1. `db.set()` inserts into the **active memtable** (a SkipList in `memtable.rs` / `skiplist.rs`)
2. When the memtable reaches its memory limit, it becomes **immutable** and a new active memtable is created
3. A background maintainer thread (10ms poll) flushes immutable memtables to **SSTables** on disk via `sstable_builder.rs` + `sstable_writer.rs`
4. Flushed SSTables are registered in the **manifest** (`manifest.rs`) at Level 0, then compaction may merge them into deeper levels

### Read Path

All reads (point lookups and range scans) use `MergeSortIterator` (`merge_sort_iterator.rs`), which performs a k-way merge across:
- The active memtable
- All immutable memtables
- All SSTable levels (via `file_block_iterator.rs` + `sstable_reader.rs`)

SSTables use an index block for binary search + a **block cache** (`block_cache.rs`) with configurable shards to avoid re-reading disk.

### Snapshot Isolation

Reads snapshot `Arc<ReadState>` (memtable refs + SSTable version) under a brief `Mutex<DBInner>` lock. The snapshot is immutable after capture, enabling concurrent reads without holding the lock.

### Compaction

The maintainer thread runs `maybe_compact` after each flush. `pick_compaction` selects a job:
- **Level 0**: triggers when file count exceeds `compaction_level0_file_count_trigger`
- **Level i > 0**: triggers when total size exceeds `compaction_level_size_base * compaction_level_size_multiplier^i`

`run_compaction` performs a k-way merge of the selected SSTables (plus the active memtable as an empty placeholder) and writes the output into Level i+1. Tombstones are dropped when compacting into the bottom level. The manifest is updated atomically, and the old SSTable files are deleted.

### Crash Recovery

On `DB::open`, the WAL is replayed before normal operation:
1. All `.wal` files in `data_dir` are scanned and parsed in order
2. Each WAL file becomes a recovered `ImmutableMemtable`; CRC mismatches at the tail are treated as partial writes and silently truncated
3. Atomic sequence counters (`log_sequence_num`, `file_sequence_num`) are restored to avoid collisions with recovered data
4. Recovered immutable memtables are flushed to SSTables by the maintainer thread as normal

The manifest also replays its on-disk log at open time to restore the SSTable version.

### Key Modules

| Module | Role |
|--------|------|
| `db.rs` | Orchestrator: public API, maintainer thread, flush, compaction, WAL recovery |
| `db_config.rs` | Configuration (memtable size, cache shards, block size, compaction thresholds, etc.) |
| `db_context.rs` | Shared config + logging helpers passed through subsystems |
| `manifest.rs` | Tracks SSTable files and metadata (key ranges, file sizes) across levels; append-only log with CRC |
| `block_cache.rs` | Sharded LRU cache keyed by `(file_id, block_id)` |
| `merge_sort_iterator.rs` | K-way merge for reads spanning memtables and all SSTable levels |
| `memory_manager.rs` | Memory pool with back-pressure: blocks writers when memory usage is too high |
| `wal.rs` | Durable write-ahead log: append records with CRC32, rotate on memtable freeze, replay on open |
