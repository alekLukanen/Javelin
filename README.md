# Javelin

Javelin is a LSM-tree based embedded key-value storage engine.

## Running Tests

To view the print statements you can run the tests like this:
```
cargo test teest_large_case_with_immutable_memtable_created -- --nocapture
```

Generate flame graph of test case:

```
cargo flamegraph --root --bin main
```


## TODO

- [x] Write tests for the sstable writer
- [x] Read sstables up on a get request
- [x] Write an algorithm to binary search the sstable using it's index block.
- [x] Read from sstables
- [x] Create a merge iterator which is able to perform a forward range scan
- [ ] Create the manifest struct and file log for changes
- [ ] Add sstable metrics to the manifest 
  - Key range, sequence range, file size
- [ ] WAL for each individual memtable, when a memtable is written to the manifest and to the sstable
then the WAL can be deleted
- [ ] Ability to replay the WAL up to the point where the memtables were at before the crash
- [ ] Compactor which can merge files from a higher level into a lower level in non-overlapping file ranges
- [ ] Concurrency manager which tracks the oldest log sequence number still in use

