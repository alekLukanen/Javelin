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
4. Flushed SSTables are registered in the **manifest** (`manifest.rs`) at Level 0

### Read Path

All reads (point lookups and range scans) use `MergeSortIterator` (`merge_sort_iterator.rs`), which performs a k-way merge across:
- The active memtable
- All immutable memtables
- All SSTable levels (via `file_block_iterator.rs` + `sstable_reader.rs`)

SSTables use an index block for binary search + a **block cache** (`block_cache.rs`) with configurable shards to avoid re-reading disk.

### Snapshot Isolation

Reads snapshot `Arc<ReadState>` (memtable refs + SSTable version) under a brief `Mutex<DBInner>` lock. The snapshot is immutable after capture, enabling concurrent reads without holding the lock.

### Key Modules

| Module | Role |
|--------|------|
| `db.rs` | Orchestrator: public API, maintainer thread, read/write coordination |
| `db_config.rs` | Configuration (memtable size, cache shards, block size, etc.) |
| `manifest.rs` | Tracks SSTable files across levels (Level 0 flush target, no compaction yet) |
| `block_cache.rs` | Sharded LRU cache keyed by `(file_id, block_id)` |
| `merge_sort_iterator.rs` | K-way merge for reads spanning memtables and all SSTable levels |
| `wal.rs` | Stub — currently only atomic LSN/file sequence counters, no durable log |

### Unimplemented

- Compaction (Level i → Level i+1 merging)
- Durable WAL (crash recovery)
- SSTable metrics in manifest (key ranges, file sizes)
