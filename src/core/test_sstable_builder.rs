use super::{
    memtable::{ImmutableMemtable, Memtable},
    test_utils::TestContext,
};

#[test]
fn test_build_from_immutable_memtable() -> Result<(), Box<dyn std::error::Error>> {
    let tc = TestContext::new();

    let memtable = Memtable::new(tc.db_context.clone(), tc.memory_manager.clone());
    let immuitable_memtable = ImmutableMemtable::new(tc.db_context.clone(), memtable);

    Ok(())
}
