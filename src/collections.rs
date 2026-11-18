//! Deterministic core collection helpers ported from Dart 3.14.0
//! (`dart:core`).
//!
//! Base checkout: `flutter-golden` HEAD
//! `ab2912035e160d9cad6306df307576da6128ebdb` (`3.50.0`), Dart SDK `3.14.0`
//! revision `fa8c1270c374c1346931583a17c3b7f278d3822d`. Source paths and
//! line numbers are relative to `bin/cache/dart-sdk/lib/`.
//!
//! Ported rules:
//! - `reduce` starts with the first element and throws on an empty iterable,
//!   while `fold` starts with caller-supplied initial value and returns it
//!   unchanged for an empty iterable (`core/iterable.dart:385-429`);
//! - `where` keeps elements in source order (`core/iterable.dart:279`),
//!   `skip`/`take` operate in iteration order (`:590,632`), and `toList`
//!   returns a fresh list (`:516`);
//! - `List.addAll` appends in iterable order (`core/list.dart:473`),
//!   `indexOf` starts at a caller-supplied index (`:537`), `removeAt` shifts
//!   later elements left (`:700`), `sublist` returns `[start, end)` (`:765`),
//!   and `reversed` yields back-to-front (`:481`);
//! - `Set.add`/`remove`/`contains` preserve uniqueness
//!   (`core/set.dart:167,194,217`), and `union`/`intersection`/`difference`
//!   follow the source set's element identity (`:298,312,326`);
//! - `Map.containsKey` checks keys only (`core/map.dart:279`), `putIfAbsent`
//!   returns the existing value without calling the factory (`:397`),
//!   `addAll` overwrites existing keys (`:411`), and `remove` returns the old
//!   value or `None` (`:425`).
//!
//! Opurom adaptation: maps and sets keep insertion order (a deterministic
//! equivalent of Dart's `LinkedHashMap`/`LinkedHashSet` iteration) and callbacks
//! are host-side Rust bodies. No wall clock, no reflection, no IR change.

use std::collections::HashMap;

/// Error produced by `reduce` on an empty iterable
/// (`core/iterable.dart:392-396`), or by a guarded iterator when the
/// underlying collection changed (`internal/iterable.dart:363-367`,
/// `core/errors.dart:626-643`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoElementError {
    /// `IterableElementError.noElement`.
    NoElement,
    /// `ConcurrentModificationError` during iteration.
    ConcurrentModification,
}

impl std::fmt::Display for NoElementError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IterableElementError.noElement")
    }
}

impl std::error::Error for NoElementError {}

/// `Iterable.reduce` (`core/iterable.dart:392-402`).
pub fn reduce<T, F>(elements: &[T], combine: F) -> Result<T, NoElementError>
where
    T: Clone,
    F: Fn(&T, &T) -> T,
{
    let (first, rest) = elements.split_first().ok_or(NoElementError::NoElement)?;
    let mut value = first.clone();
    for element in rest {
        value = combine(&value, element);
    }
    Ok(value)
}

/// `Iterable.fold` (`core/iterable.dart:425-429`).
pub fn fold<T, R, F>(elements: &[T], initial: R, combine: F) -> R
where
    F: Fn(R, &T) -> R,
{
    let mut value = initial;
    for element in elements {
        value = combine(value, element);
    }
    value
}

/// `Iterable.where` (`core/iterable.dart:279`): keeps order.
pub fn r#where<T, F>(elements: &[T], test: F) -> Vec<T>
where
    T: Clone,
    F: Fn(&T) -> bool,
{
    elements
        .iter()
        .filter(|element| test(element))
        .cloned()
        .collect()
}

/// `Iterable.map` (`core/iterable.dart:257`): one output per input, in order.
pub fn map<T, R, F>(elements: &[T], convert: F) -> Vec<R>
where
    F: Fn(&T) -> R,
{
    elements.iter().map(convert).collect()
}

/// `Iterable.skip` (`core/iterable.dart:632`) and `take` (`:590`).
pub fn skip_take<T>(elements: &[T], skip: usize, take: Option<usize>) -> Vec<T>
where
    T: Clone,
{
    let start = skip.min(elements.len());
    let count = elements.len() - start;
    let count = take.map_or(count, |limit| count.min(limit));
    elements[start..start + count].to_vec()
}

/// `Iterable.toList` (`core/iterable.dart:516`): a fresh list.
pub fn to_list<T: Clone>(elements: &[T]) -> Vec<T> {
    elements.to_vec()
}

/// `List.addAll` (`core/list.dart:473`).
pub fn add_all<T: Clone>(target: &mut Vec<T>, elements: &[T]) {
    target.extend_from_slice(elements);
}

/// `List.indexOf` (`core/list.dart:537`): first match at or after `start`.
pub fn index_of<T: PartialEq>(elements: &[T], value: &T, start: usize) -> Option<usize> {
    if start >= elements.len() {
        return None;
    }
    elements[start..]
        .iter()
        .position(|element| element == value)
        .map(|index| index + start)
}

/// `List.lastIndexOf` (`core/list.dart:602`): last match at or before `start`.
pub fn last_index_of<T: PartialEq>(
    elements: &[T],
    value: &T,
    start: Option<usize>,
) -> Option<usize> {
    if elements.is_empty() {
        return None;
    }
    let end = start.unwrap_or(elements.len() - 1).min(elements.len() - 1);
    elements[..=end]
        .iter()
        .rposition(|element| element == value)
}

/// `List.removeAt` (`core/list.dart:700`): removes and shifts left.
pub fn remove_at<T>(elements: &mut Vec<T>, index: usize) -> Option<T> {
    if index < elements.len() {
        Some(elements.remove(index))
    } else {
        None
    }
}

/// `List.sublist` (`core/list.dart:765`): the half-open range `[start, end)`.
pub fn sublist<T: Clone>(elements: &[T], start: usize, end: Option<usize>) -> Vec<T> {
    let end = end.unwrap_or(elements.len()).min(elements.len());
    if start >= end {
        Vec::new()
    } else {
        elements[start..end].to_vec()
    }
}

/// `List.reversed` (`core/list.dart:481`).
pub fn reversed<T: Clone>(elements: &[T]) -> Vec<T> {
    let mut out = elements.to_vec();
    out.reverse();
    out
}

/// An insertion-ordered set mirroring Dart's `LinkedHashSet` iteration
/// (`core/set.dart:167-326`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedSet<T: Eq + std::hash::Hash> {
    order: Vec<T>,
    members: HashMap<T, ()>,
}

impl<T: Eq + std::hash::Hash + Clone> Default for OrderedSet<T> {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            members: HashMap::new(),
        }
    }
}

impl<T: Eq + std::hash::Hash + Clone> OrderedSet<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// `Set.add` (`core/set.dart:194`): returns true only when newly inserted.
    pub fn add(&mut self, value: T) -> bool {
        if self.members.insert(value.clone(), ()).is_none() {
            self.order.push(value);
            true
        } else {
            false
        }
    }

    /// `Set.remove` (`core/set.dart:217`).
    pub fn remove(&mut self, value: &T) -> bool {
        if self.members.remove(value).is_some() {
            self.order.retain(|element| element != value);
            true
        } else {
            false
        }
    }

    /// `Set.contains` (`core/set.dart:167`).
    pub fn contains(&self, value: &T) -> bool {
        self.members.contains_key(value)
    }

    /// `Set.addAll` (`core/set.dart:205`).
    pub fn add_all(&mut self, values: &[T]) {
        for value in values {
            self.add(value.clone());
        }
    }

    /// `Set.union` (`core/set.dart:312`).
    pub fn union(&self, other: &Self) -> Self {
        let mut out = self.clone();
        out.add_all(&other.order);
        out
    }

    /// `Set.intersection` (`core/set.dart:298`).
    pub fn intersection(&self, other: &Self) -> Self {
        self.order
            .iter()
            .filter(|value| other.contains(value))
            .cloned()
            .collect()
    }

    /// `Set.difference` (`core/set.dart:326`).
    pub fn difference(&self, other: &Self) -> Self {
        self.order
            .iter()
            .filter(|value| !other.contains(value))
            .cloned()
            .collect()
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &T> {
        self.order.iter()
    }

    pub fn to_vec(&self) -> Vec<T> {
        self.order.clone()
    }
}

impl<T: Eq + std::hash::Hash + Clone> FromIterator<T> for OrderedSet<T> {
    fn from_iter<I: IntoIterator<Item = T>>(iter: I) -> Self {
        let mut out = Self::new();
        for value in iter {
            out.add(value);
        }
        out
    }
}

/// An insertion-ordered map mirroring Dart's `LinkedHashMap` iteration
/// (`core/map.dart:279-475`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderedMap<K: Eq + std::hash::Hash + Clone, V: Clone> {
    order: Vec<K>,
    values: HashMap<K, V>,
}

impl<K: Eq + std::hash::Hash + Clone, V: Clone> Default for OrderedMap<K, V> {
    fn default() -> Self {
        Self {
            order: Vec::new(),
            values: HashMap::new(),
        }
    }
}

impl<K: Eq + std::hash::Hash + Clone, V: Clone> OrderedMap<K, V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Inserts or overwrites a key, preserving the key's first insertion
    /// position.
    pub fn insert(&mut self, key: K, value: V) -> Option<V> {
        let previous = self.values.insert(key.clone(), value);
        if previous.is_none() {
            self.order.push(key);
        }
        previous
    }

    /// `Map.containsKey` (`core/map.dart:279`).
    pub fn contains_key(&self, key: &K) -> bool {
        self.values.contains_key(key)
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.values.get(key)
    }

    /// `Map.putIfAbsent` (`core/map.dart:397`): the factory is called only
    /// when the key is absent.
    pub fn put_if_absent<F: FnOnce() -> V>(&mut self, key: K, factory: F) -> V {
        if let Some(value) = self.values.get(&key) {
            return value.clone();
        }
        let value = factory();
        self.insert(key, value.clone());
        value
    }

    /// `Map.addAll` (`core/map.dart:411`): existing keys are overwritten.
    pub fn add_all(&mut self, other: &Self) {
        for key in &other.order {
            let value = other.values.get(key).unwrap().clone();
            self.insert(key.clone(), value);
        }
    }

    /// `Map.remove` (`core/map.dart:425`): the old value is returned.
    pub fn remove(&mut self, key: &K) -> Option<V> {
        let previous = self.values.remove(key);
        if previous.is_some() {
            self.order.retain(|existing| existing != key);
        }
        previous
    }

    /// `Map.clear` (`core/map.dart:434`).
    pub fn clear(&mut self) {
        self.order.clear();
        self.values.clear();
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &K> {
        self.order.iter()
    }

    pub fn values(&self) -> impl Iterator<Item = &V> {
        self.order.iter().map(|key| self.values.get(key).unwrap())
    }

    pub fn entries(&self) -> impl Iterator<Item = (&K, &V)> {
        self.order
            .iter()
            .map(|key| (key, self.values.get(key).unwrap()))
    }
}

/// A deterministic iterator with Dart's length-change guard
/// (`internal/iterable.dart:346-375`, `core/iterable.dart:54-61`).
///
/// The length is captured at construction. A later `move_next` checks the
/// current length and returns `ConcurrentModificationError` if it changed,
/// exactly as Dart's `ListIterator.moveNext` does.
pub struct GuardedIterator<'a, T> {
    elements: &'a [T],
    captured_length: usize,
    /// The host reports the live collection length through this cell when
    /// the collection can change during iteration. A plain slice iterator
    /// never changes it.
    live_length: Option<&'a std::cell::Cell<usize>>,
    index: usize,
}

impl<'a, T> GuardedIterator<'a, T> {
    pub fn new(elements: &'a [T]) -> Self {
        Self {
            elements,
            captured_length: elements.len(),
            live_length: None,
            index: 0,
        }
    }

    /// Attaches a live length observer, used by hosts whose collection can
    /// be mutated while the iterator exists (`core/iterable.dart:54-58`).
    pub fn with_live_length(mut self, live: &'a std::cell::Cell<usize>) -> Self {
        self.live_length = Some(live);
        self
    }

    /// `ListIterator.moveNext` (`internal/iterable.dart:363-375`).
    pub fn move_next(&mut self) -> Result<Option<&'a T>, NoElementError> {
        let live = self
            .live_length
            .map_or(self.elements.len(), |cell| cell.get());
        if live != self.captured_length {
            return Err(NoElementError::ConcurrentModification);
        }
        if self.index >= self.elements.len() {
            return Ok(None);
        }
        let current = &self.elements[self.index];
        self.index += 1;
        Ok(Some(current))
    }

    pub fn index(&self) -> usize {
        self.index
    }
}

/// Sorts using Dart's comparator contract (`core/list.dart:485-511`). The
/// comparator's error is propagated unchanged; because Dart's sort is not
/// guaranteed stable (`list.dart:502-505`), equal elements may be reordered.
pub fn sort_by<T, F>(elements: &mut [T], compare: F)
where
    F: Fn(&T, &T) -> std::cmp::Ordering,
{
    // Rust's stable sort is stricter than Dart's requirement but still
    // satisfies the comparator contract; a panicking comparator propagates
    // to the caller just like Dart's.
    elements.sort_by(compare);
}
