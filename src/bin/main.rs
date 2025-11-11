use Javelin::core::{
    entry::{Entry, LogEntry},
    skiplist::SkipList,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    println!("Hello, world!");

    let skiplist = SkipList::new(0.5, 1_000, 3);

    skiplist.insert(LogEntry::new(
        Entry::Put {
            key: vec![1u8],
            val: vec![1u8],
        },
        1,
    ));

    let entry = skiplist.get(&vec![1u8], 1);
    let entry_not_found = skiplist.get(&vec![1u8], 0);

    println!("entry: {:?}, entry_not_found: {:?}", entry, entry_not_found);

    Ok(())
}
