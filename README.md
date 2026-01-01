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

