mod append;
mod borrow;
mod dedup_sorted_iter;
mod fix;
pub mod map;
mod mem;
mod merge_iter;
mod navigate;
mod node;
mod remove;
mod search;
//pub mod set;
mod set_val;
mod split;

#[doc(hidden)]
trait Recover<C> {
    type Key;

    fn get(&self, comp: C) -> Option<&Self::Key>;
    fn take(&mut self, comp: C) -> Option<Self::Key>;
    fn replace(&mut self, key: Self::Key, comp: C) -> Option<Self::Key>;
}
