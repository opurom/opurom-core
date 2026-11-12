use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::hash::Hash;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceError {
    BudgetExceeded { required: usize, budget: usize },
    InvalidValue(String),
}

impl fmt::Display for ResourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BudgetExceeded { required, budget } => {
                write!(f, "{required} bytes needed, budget is {budget} bytes")
            }
            Self::InvalidValue(m) => write!(f, "invalid cached value: {m}"),
        }
    }
}

impl std::error::Error for ResourceError {}

/// Bounded LRU cache. Zero-byte entries are charged one byte so the entry count stays bounded.
#[derive(Debug, Clone)]
pub struct ResourceManager<K, V> {
    budget: usize,
    used_bytes: usize,
    entries: HashMap<K, (V, usize)>,
    order: VecDeque<K>,
}

impl<K, V> ResourceManager<K, V>
where
    K: Clone + Eq + Hash,
{
    pub fn new(budget: usize) -> Self {
        ResourceManager {
            budget,
            used_bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    pub fn used_bytes(&self) -> usize {
        self.used_bytes
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn keys_lru(&self) -> Vec<K> {
        self.order.iter().cloned().collect()
    }

    pub fn insert(&mut self, key: K, value: V, size: usize) -> Result<Option<V>, ResourceError> {
        let charge = size.max(1);
        if charge > self.budget {
            return Err(ResourceError::BudgetExceeded {
                required: charge,
                budget: self.budget,
            });
        }
        let replaced = self.evict(&key);
        while self.used_bytes > self.budget - charge {
            let Some(oldest) = self.order.pop_front() else {
                return Err(ResourceError::BudgetExceeded {
                    required: charge,
                    budget: self.budget,
                });
            };
            if let Some((_, evicted_size)) = self.entries.remove(&oldest) {
                self.used_bytes = self.used_bytes.saturating_sub(evicted_size);
            }
        }
        self.entries.insert(key.clone(), (value, charge));
        self.order.push_back(key);
        self.used_bytes += charge;
        Ok(replaced)
    }

    pub fn evict(&mut self, key: &K) -> Option<V> {
        let (value, charge) = self.entries.remove(key)?;
        self.remove_from_order(key);
        self.used_bytes = self.used_bytes.saturating_sub(charge);
        Some(value)
    }

    fn remove_from_order(&mut self, key: &K) {
        if let Some(position) = self.order.iter().position(|candidate| candidate == key) {
            self.order.remove(position);
        }
    }
}

impl<K, V> ResourceManager<K, V>
where
    K: Clone + Eq + Hash,
    V: Clone,
{
    pub fn get(&mut self, key: &K) -> Option<V> {
        let value = self.entries.get(key)?.0.clone();
        self.remove_from_order(key);
        self.order.push_back(key.clone());
        Some(value)
    }
}
