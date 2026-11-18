//! Regression tests for the core collection helpers, pinned to Dart 3.14.0
//! (`flutter-golden` HEAD ab2912035e160d9cad6306df307576da6128ebdb).

use opurom_core::{
    add_all, collect_where, fold, index_of, last_index_of, map, reduce, remove_at, reversed,
    skip_take, sort_by, sublist, to_list, GuardedIterator, NoElementError, OrderedMap, OrderedSet,
};

#[test]
fn reduce_uses_first_element_and_folds_the_rest() {
    // core/iterable.dart:392-402.
    assert_eq!(reduce(&[10, 2, 5, 1], |a, b| a + b), Ok(18));
    assert_eq!(
        reduce::<i64, _>(&[], |a, b| a + b),
        Err(NoElementError::NoElement)
    );
}

#[test]
fn fold_uses_initial_value_and_survives_empty() {
    // core/iterable.dart:425-429.
    assert_eq!(fold(&[10, 2, 5], 100, |a, b| a + b), 117);
    assert_eq!(fold::<i64, i64, _>(&[], 42, |a, b| a + b), 42);
}

#[test]
fn where_keeps_order_and_map_transforms_each_element() {
    // core/iterable.dart:257,279.
    assert_eq!(
        collect_where(&[1, 2, 3, 4], |value| value % 2 == 0),
        vec![2, 4]
    );
    assert_eq!(map(&[1, 2, 3], |value| value * 2), vec![2, 4, 6]);
}

#[test]
fn skip_take_follow_iteration_order() {
    // core/iterable.dart:590,632.
    assert_eq!(skip_take(&[1, 2, 3, 4, 5], 1, Some(3)), vec![2, 3, 4]);
    assert_eq!(skip_take(&[1, 2, 3], 10, Some(2)), Vec::<i64>::new());
    assert_eq!(skip_take(&[1, 2, 3], 1, None), vec![2, 3]);
}

#[test]
fn list_helpers_match_dart_semantics() {
    // core/list.dart:473,537,602,700,765,481.
    let mut values = vec![1, 2, 3];
    add_all(&mut values, &[4, 5]);
    assert_eq!(values, vec![1, 2, 3, 4, 5]);
    assert_eq!(index_of(&values, &3, 0), Some(2));
    assert_eq!(index_of(&values, &3, 3), None);
    assert_eq!(last_index_of(&values, &3, None), Some(2));
    assert_eq!(remove_at(&mut values, 1), Some(2));
    assert_eq!(values, vec![1, 3, 4, 5]);
    assert_eq!(sublist(&[1, 2, 3, 4], 1, Some(3)), vec![2, 3]);
    assert_eq!(sublist(&[1, 2, 3, 4], 1, None), vec![2, 3, 4]);
    assert_eq!(reversed(&[1, 2, 3]), vec![3, 2, 1]);
    assert_eq!(to_list(&[1, 2]), vec![1, 2]);
}

#[test]
fn ordered_set_keeps_uniqueness_and_insertion_order() {
    // core/set.dart:167,194,205,217.
    let mut set = OrderedSet::new();
    assert!(set.add(3));
    assert!(set.add(1));
    assert!(!set.add(3));
    assert!(set.contains(&1));
    set.add_all(&[2, 1]);
    assert_eq!(set.to_vec(), vec![3, 1, 2]);
    assert!(set.remove(&1));
    assert!(!set.remove(&1));
    assert_eq!(set.len(), 2);
}

#[test]
fn set_algebra_preserves_source_order() {
    // core/set.dart:298,312,326.
    let a: OrderedSet<i64> = [1, 2, 3].into_iter().collect();
    let b: OrderedSet<i64> = [3, 4, 2].into_iter().collect();
    assert_eq!(a.union(&b).to_vec(), vec![1, 2, 3, 4]);
    assert_eq!(a.intersection(&b).to_vec(), vec![2, 3]);
    assert_eq!(a.difference(&b).to_vec(), vec![1]);
}

#[test]
fn ordered_map_put_if_absent_and_add_all_follow_dart() {
    // core/map.dart:279,397,411,425,434.
    let mut map = OrderedMap::new();
    assert_eq!(map.put_if_absent(1, || 10), 10);
    assert_eq!(map.put_if_absent(1, || 99), 10);
    assert!(map.contains_key(&1));
    let mut other = OrderedMap::new();
    other.insert(2, 20);
    other.insert(1, 100);
    map.add_all(&other);
    assert_eq!(map.get(&1), Some(&100));
    assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![1, 2]);
    assert_eq!(map.remove(&1), Some(100));
    assert_eq!(map.keys().copied().collect::<Vec<_>>(), vec![2]);
    map.clear();
    assert!(map.is_empty());
}

#[test]
fn map_entries_values_follow_key_order() {
    // core/map.dart:297,462,475.
    let mut map = OrderedMap::new();
    map.insert("b", 2);
    map.insert("a", 1);
    assert_eq!(map.values().copied().collect::<Vec<_>>(), vec![2, 1]);
    assert_eq!(
        map.entries().map(|(key, _)| *key).collect::<Vec<_>>(),
        vec!["b", "a"]
    );
}

#[test]
fn collection_results_are_byte_deterministic() {
    let values = [3, 1, 4, 1, 5];
    let a: OrderedSet<i64> = values.into_iter().collect();
    let b: OrderedSet<i64> = values.into_iter().collect();
    assert_eq!(a, b);
    assert_eq!(fold(&values, 0, |sum, value| sum + value), 14);
}

#[test]
fn reduce_empty_uses_typed_no_element_variant() {
    assert_eq!(
        reduce::<i64, _>(&[], |a, b| a + b),
        Err(NoElementError::NoElement)
    );
    assert_eq!(
        NoElementError::NoElement.to_string(),
        "IterableElementError.noElement"
    );
}

#[test]
fn guarded_iterator_walks_empty_and_single_element() {
    // internal/iterable.dart:363-375: moveNext returns false past the end.
    let empty: Vec<i64> = Vec::new();
    let mut iterator = GuardedIterator::new(&empty);
    assert_eq!(iterator.move_next(), Ok(None));
    assert_eq!(iterator.index(), 0);

    let single = vec![7];
    let mut iterator = GuardedIterator::new(&single);
    assert_eq!(iterator.move_next(), Ok(Some(&7)));
    assert_eq!(iterator.move_next(), Ok(None));
    assert_eq!(iterator.index(), 1);
}

#[test]
fn guarded_iterator_detects_length_change() {
    // core/iterable.dart:54-58, internal/iterable.dart:363-367: changing a
    // collection during iteration is reported at the next moveNext.
    let values = vec![1, 2, 3];
    let live_length = std::cell::Cell::new(values.len());
    let mut iterator = GuardedIterator::new(&values).with_live_length(&live_length);
    assert_eq!(iterator.move_next(), Ok(Some(&1)));
    live_length.set(4); // host mutates the collection
    assert_eq!(
        iterator.move_next(),
        Err(NoElementError::ConcurrentModification)
    );
}

#[test]
fn guarded_iterator_detects_shrink() {
    let values = vec![1, 2, 3];
    let live_length = std::cell::Cell::new(values.len());
    let mut iterator = GuardedIterator::new(&values).with_live_length(&live_length);
    live_length.set(2);
    assert_eq!(
        iterator.move_next(),
        Err(NoElementError::ConcurrentModification)
    );
}

#[test]
fn large_sequential_iteration_is_exact() {
    // core/iterable.dart:69-88: operators iterate in source order.
    let values: Vec<i64> = (0..10_000).collect();
    let mut iterator = GuardedIterator::new(&values);
    let mut count = 0i64;
    while let Ok(Some(value)) = iterator.move_next() {
        assert_eq!(*value, count);
        count += 1;
    }
    assert_eq!(count, 10_000);
}

#[test]
fn duplicates_are_preserved_by_iterables_but_removed_by_sets() {
    // core/iterable.dart:257,279: map/where keep duplicates; core/set.dart:194
    // enforces uniqueness.
    let values = [1, 1, 2, 2, 2, 3];
    assert_eq!(collect_where(&values, |_| true), values);
    let set: OrderedSet<i64> = values.into_iter().collect();
    assert_eq!(set.to_vec(), vec![1, 2, 3]);
    assert_eq!(set.len(), 3);
}

#[test]
fn comparator_errors_propagate_without_swallowing() {
    // core/list.dart:485-511: the comparator must act as a Comparator and
    // its failure surfaces to the caller.
    let result = std::panic::catch_unwind(|| {
        let mut values = vec![3, 1, 2];
        sort_by(&mut values, |left, right| {
            if *left == 1 {
                panic!("comparator failed");
            }
            left.cmp(right)
        });
    });
    assert!(result.is_err());
}

#[test]
fn comparator_sorts_extreme_and_duplicate_values() {
    // core/list.dart:485-500: a comparator defines the result order.
    let mut values = vec![i64::MAX, 0, i64::MIN, -1, 0];
    sort_by(&mut values, |left, right| left.cmp(right));
    assert_eq!(values, vec![i64::MIN, -1, 0, 0, i64::MAX]);
}

#[test]
fn map_and_add_all_do_not_alias_source() {
    // core/iterable.dart:257, core/list.dart:473: results are fresh lists.
    let source = vec![1, 2, 3];
    let mapped = map(&source, |value| value * 2);
    let mut target = Vec::new();
    add_all(&mut target, &mapped);
    assert_eq!(target, vec![2, 4, 6]);
    assert_eq!(source, vec![1, 2, 3]);
}

#[test]
fn ordered_set_empty_set_algebra() {
    // core/set.dart:298,312,326: empty sets are identities for union and
    // annihilators for intersection/difference.
    let empty = OrderedSet::<i64>::new();
    let full: OrderedSet<i64> = [1, 2].into_iter().collect();
    assert_eq!(full.union(&empty).to_vec(), vec![1, 2]);
    assert!(full.intersection(&empty).is_empty());
    assert_eq!(full.difference(&empty).to_vec(), vec![1, 2]);
    assert!(empty.union(&full).to_vec() == vec![1, 2]);
}

#[test]
fn ordered_map_large_insertion_and_removal_keeps_order() {
    // core/map.dart:397,411,425: insertion order remains stable across
    // overwrite and removal.
    let mut map = OrderedMap::new();
    for value in 0..1_000 {
        map.insert(value, value * 2);
    }
    map.insert(500, -1);
    assert_eq!(map.len(), 1_000);
    assert_eq!(map.get(&500), Some(&-1));
    assert_eq!(
        map.keys().copied().collect::<Vec<_>>(),
        (0..1_000).collect::<Vec<_>>()
    );
    assert_eq!(map.remove(&0), Some(0));
    assert_eq!(map.keys().next(), Some(&1));
}
