use crate::{SortableByWithOrder, TotalOrder};
use core::cmp::Ordering;
use core::ops::{Bound, RangeBounds};

use super::node::{marker, ForceResult::*, Handle, NodeRef};

use cfg_if::cfg_if;
use SearchBound::*;
use SearchResult::*;

pub enum SearchBound {
    /// An inclusive bound to look for, just like `Bound::Included(T)`.
    Included,
    /// An exclusive bound to look for, just like `Bound::Excluded(T)`.
    Excluded,
    /// An unconditional inclusive bound, just like `Bound::Unbounded`.
    AllIncluded,
    /// An unconditional exclusive bound.
    AllExcluded,
}

impl SearchBound {
    pub fn from_range<T>(range_bound: Bound<T>) -> Self {
        match range_bound {
            Bound::Included(_) => Included,
            Bound::Excluded(_) => Excluded,
            Bound::Unbounded => AllIncluded,
        }
    }
}

pub enum SearchResult<BorrowType, K, V, FoundType, GoDownType> {
    Found(Handle<NodeRef<BorrowType, K, V, FoundType>, marker::KV>),
    GoDown(Handle<NodeRef<BorrowType, K, V, GoDownType>, marker::Edge>),
}

pub enum IndexResult {
    KV(usize),
    Edge(usize),
}

impl<BorrowType: marker::BorrowType, K, V> NodeRef<BorrowType, K, V, marker::LeafOrInternal> {
    /// Looks up a given key in a (sub)tree headed by the node, recursively.
    /// Returns a `Found` with the handle of the matching KV, if any. Otherwise,
    /// returns a `GoDown` with the handle of the leaf edge where the key belongs.
    ///
    /// The result is meaningful only if the tree is ordered by key, like the tree
    /// in a `BTreeMap` is.
    pub fn search_tree<C>(
        mut self,
        comp: C,
    ) -> SearchResult<BorrowType, K, V, marker::LeafOrInternal, marker::Leaf>
    where
        C: FnMut(&K) -> Ordering,
    {
        loop {
            self = match self.search_node(comp) {
                Found(handle) => return Found(handle),
                GoDown(handle) => match handle.force() {
                    Leaf(leaf) => return GoDown(leaf),
                    Internal(internal) => internal.descend(),
                },
            }
        }
    }

    /// Descends to the nearest node where the edge matching the lower bound
    /// of the range is different from the edge matching the upper bound, i.e.,
    /// the nearest node that has at least one key contained in the range.
    ///
    /// If found, returns an `Ok` with that node, the strictly ascending pair of
    /// edge indices in the node delimiting the range, and the corresponding
    /// pair of bounds for continuing the search in the child nodes, in case
    /// the node is internal.
    ///
    /// If not found, returns an `Err` with the leaf edge matching the entire
    /// range.
    ///
    /// As a diagnostic service, panics if the range specifies impossible bounds.
    ///
    /// The result is meaningful only if the tree is ordered by key.
    pub fn search_tree_for_bifurcation<C1, C2>(
        mut self,
        lower_comp: C1,
        mut lower_bound: SearchBound,
        upper_comp: C2,
        mut upper_bound: SearchBound,
    ) -> Result<
        (NodeRef<BorrowType, K, V, marker::LeafOrInternal>, usize, usize, SearchBound, SearchBound),
        Handle<NodeRef<BorrowType, K, V, marker::Leaf>, marker::Edge>,
    >
    where
        C1: FnMut(&K) -> Ordering,
        C2: FnMut(&K) -> Ordering,
    {
        // Determine if map or set is being searched
        #[cfg(feature = "specialization")]
        let is_set = <V as super::set_val::IsSetVal>::is_set_val();

        loop {
            let (lower_edge_idx, lower_child_bound) =
                self.find_lower_bound_index(lower_comp, lower_bound);
            let (upper_edge_idx, upper_child_bound) =
                unsafe { self.find_upper_bound_index(upper_comp, upper_bound, lower_edge_idx) };
            if lower_edge_idx < upper_edge_idx {
                return Ok((
                    self,
                    lower_edge_idx,
                    upper_edge_idx,
                    lower_child_bound,
                    upper_child_bound,
                ));
            }
            debug_assert_eq!(lower_edge_idx, upper_edge_idx);
            let common_edge = unsafe { Handle::new_edge(self, lower_edge_idx) };
            match common_edge.force() {
                Leaf(common_edge) => return Err(common_edge),
                Internal(common_edge) => {
                    self = common_edge.descend();
                    lower_bound = lower_child_bound;
                    upper_bound = upper_child_bound;
                }
            }
        }
    }

    /// Finds an edge in the node delimiting the lower bound of a range.
    /// Also returns the lower bound to be used for continuing the search in
    /// the matching child node, if `self` is an internal node.
    ///
    /// The result is meaningful only if the tree is ordered by key.
    pub fn find_lower_bound_edge<C>(
        self,
        comp: C,
        bound: SearchBound,
    ) -> (Handle<Self, marker::Edge>, SearchBound)
    where
        C: FnMut(&K) -> Ordering,
    {
        let (edge_idx, bound) = self.find_lower_bound_index(comp, bound);
        let edge = unsafe { Handle::new_edge(self, edge_idx) };
        (edge, bound)
    }

    /// Clone of `find_lower_bound_edge` for the upper bound.
    pub fn find_upper_bound_edge<C>(
        self,
        comp: C,
        bound: SearchBound
    ) -> (Handle<Self, marker::Edge>, SearchBound)
    where
        C: FnMut(&K) -> Ordering,
    {
        let (edge_idx, bound) = unsafe { self.find_upper_bound_index(comp, bound, 0) };
        let edge = unsafe { Handle::new_edge(self, edge_idx) };
        (edge, bound)
    }
}

impl<BorrowType, K, V, Type> NodeRef<BorrowType, K, V, Type> {
    /// Looks up a given key in the node, without recursion.
    /// Returns a `Found` with the handle of the matching KV, if any. Otherwise,
    /// returns a `GoDown` with the handle of the edge where the key might be found
    /// (if the node is internal) or where the key can be inserted.
    ///
    /// The result is meaningful only if the tree is ordered by key, like the tree
    /// in a `BTreeMap` is.
    pub fn search_node<C>(self, comp: C) -> SearchResult<BorrowType, K, V, Type, Type>
    where
        C: FnMut(&K) -> Ordering,
    {
        match unsafe { self.find_key_index(comp, 0) } {
            IndexResult::KV(idx) => Found(unsafe { Handle::new_kv(self, idx) }),
            IndexResult::Edge(idx) => GoDown(unsafe { Handle::new_edge(self, idx) }),
        }
    }

    /// Returns either the KV index in the node at which the key (or an equivalent)
    /// exists, or the edge index where the key belongs, starting from a particular index.
    ///
    /// The result is meaningful only if the tree is ordered by key, like the tree
    /// in a `BTreeMap` is.
    ///
    /// # Safety
    /// `start_index` must be a valid edge index for the node.
    unsafe fn find_key_index<C>(&self, mut comp: C, start_index: usize) -> IndexResult
    where
        C: FnMut(&K) -> Ordering,
    {
        let node = self.reborrow();
        let keys = node.keys();
        debug_assert!(start_index <= keys.len());
        for (offset, k) in unsafe { keys.get_unchecked(start_index..) }.iter().enumerate() {
            match comp(k) {
                Ordering::Greater => {}
                Ordering::Equal => return IndexResult::KV(start_index + offset),
                Ordering::Less => return IndexResult::Edge(start_index + offset),
            }
        }
        IndexResult::Edge(keys.len())
    }

    /// Finds an edge index in the node delimiting the lower bound of a range.
    /// Also returns the lower bound to be used for continuing the search in
    /// the matching child node, if `self` is an internal node.
    ///
    /// The result is meaningful only if the tree is ordered by key.
    fn find_lower_bound_index<C>(&self, comp: C, bound: SearchBound) -> (usize, SearchBound)
    where
        C: FnMut(&K) -> Ordering,
    {
        match bound {
            Included => match unsafe { self.find_key_index(comp, 0) } {
                IndexResult::KV(idx) => (idx, AllExcluded),
                IndexResult::Edge(idx) => (idx, bound),
            },
            Excluded => match unsafe { self.find_key_index(comp, 0) } {
                IndexResult::KV(idx) => (idx + 1, AllIncluded),
                IndexResult::Edge(idx) => (idx, bound),
            },
            AllIncluded => (0, AllIncluded),
            AllExcluded => (self.len(), AllExcluded),
        }
    }

    /// Mirror image of `find_lower_bound_index` for the upper bound,
    /// with an additional parameter to skip part of the key array.
    ///
    /// # Safety
    /// `start_index` must be a valid edge index for the node.
    unsafe fn find_upper_bound_index<C>(
        &self,
        comp: C,
        bound: SearchBound,
        start_index: usize,
    ) -> (usize, SearchBound)
    where
        C: FnMut(&K) -> Ordering,
    {
        match bound {
            Included => match unsafe { self.find_key_index(comp, start_index) } {
                IndexResult::KV(idx) => (idx + 1, AllExcluded),
                IndexResult::Edge(idx) => (idx, bound),
            },
            Excluded => match unsafe { self.find_key_index(comp, start_index) } {
                IndexResult::KV(idx) => (idx, AllIncluded),
                IndexResult::Edge(idx) => (idx, bound),
            },
            AllIncluded => (self.len(), AllIncluded),
            AllExcluded => (start_index, AllExcluded),
        }
    }
}
