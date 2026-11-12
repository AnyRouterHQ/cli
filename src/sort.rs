//! One shared stable sort, and `BTreeMap` / `BTreeSet` built by insertion.
//!
//! Every `sort_by` call site with its own closure, and every `collect` into a
//! `BTreeMap` or `BTreeSet` (which sort their input first), compiles its own copy of the
//! standard library's driftsort: about 3.5 KB each, roughly 80 KB across this
//! crate. These helpers keep one copy: the elements are ordered through a
//! sort of their indices with a type-erased comparator, then moved into
//! place. The result is the same as `slice::sort_by`, ties included.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

/// Stable sort by `cmp`; same result as `slice::sort_by`.
pub fn sort_by<T>(v: &mut [T], cmp: impl Fn(&T, &T) -> Ordering) {
    let order = sorted_indices(v.len(), &|a, b| cmp(&v[a], &v[b]));
    apply(v, order);
}

/// Stable sort by key; same result as `slice::sort_by_key`.
pub fn sort_by_key<T, K: Ord>(v: &mut [T], key: impl Fn(&T) -> K) {
    sort_by(v, |a, b| key(a).cmp(&key(b)));
}

/// Same result as `slice::sort`.
pub fn sort<T: Ord>(v: &mut [T]) {
    sort_by(v, T::cmp);
}

/// `BTreeMap` from pairs, a later duplicate key replacing the earlier value
/// (as `collect` does), built by insertion so no sort is compiled in.
pub fn map<K: Ord, V>(pairs: impl IntoIterator<Item = (K, V)>) -> BTreeMap<K, V> {
    let mut out = BTreeMap::new();
    for (k, v) in pairs {
        out.insert(k, v);
    }
    out
}

/// `BTreeSet` built by insertion, so no sort is compiled in; same set as
/// `collect`.
pub fn set<T: Ord>(items: impl IntoIterator<Item = T>) -> BTreeSet<T> {
    let mut out = BTreeSet::new();
    for item in items {
        out.insert(item);
    }
    out
}

/// The one compiled sort: positions `0..len` in stable order.
fn sorted_indices(len: usize, cmp: &dyn Fn(usize, usize) -> Ordering) -> Vec<usize> {
    let mut order: Vec<usize> = (0..len).collect();
    order.sort_by(|&a, &b| cmp(a, b));
    order
}

/// Rearrange `v` so `v[k]` becomes the old `v[order[k]]`, following each
/// cycle of the permutation with swaps.
fn apply<T>(v: &mut [T], mut order: Vec<usize>) {
    for start in 0..order.len() {
        let mut k = start;
        while order[k] != usize::MAX {
            let src = order[k];
            order[k] = usize::MAX;
            if src == start {
                break;
            }
            v.swap(k, src);
            k = src;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small deterministic generator, so the test needs no dependency.
    fn lcg(seed: &mut u64) -> u64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        *seed >> 33
    }

    #[test]
    fn matches_the_standard_stable_sort_ties_included() {
        let mut seed = 7;
        for len in 0..200 {
            // Few distinct keys, so most elements tie and stability shows.
            let items: Vec<(u64, usize)> = (0..len).map(|i| (lcg(&mut seed) % 5, i)).collect();
            let mut want = items.clone();
            want.sort_by_key(|a| a.0);
            let mut got = items.clone();
            sort_by(&mut got, |a, b| a.0.cmp(&b.0));
            assert_eq!(got, want, "len {len}");
            let mut got = items.clone();
            sort_by_key(&mut got, |a| std::cmp::Reverse(a.0));
            let mut want = items;
            want.sort_by_key(|a| std::cmp::Reverse(a.0));
            assert_eq!(got, want, "len {len}");
        }
    }

    #[test]
    fn sorts_owned_values_without_losing_any() {
        let mut v: Vec<String> = ["b", "a", "c", "a"].map(String::from).to_vec();
        sort(&mut v);
        assert_eq!(v, ["a", "a", "b", "c"]);
    }

    #[test]
    fn map_keeps_the_last_value_of_a_duplicate_key_like_collect() {
        let pairs = [("a", 1), ("b", 2), ("a", 3)];
        let want: BTreeMap<_, _> = pairs.into_iter().collect();
        assert_eq!(map(pairs), want);
        assert_eq!(map(pairs)["a"], 3);
    }
}
