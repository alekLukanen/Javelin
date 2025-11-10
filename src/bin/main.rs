use Javelin::core::skiplist::SkipList;

fn main() {
    println!("Hello, world!");

    let skiplist = SkipList::new(0.5, 1_000, 3);
}
