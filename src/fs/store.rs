use alloc::string::String;
use alloc::vec::Vec;
use alloc::sync::Arc;
use alloc::collections::BTreeMap;
use spin::{Mutex, Lazy};
use crate::fs::ramfs::RamDomain;

/// The Global Object Store managing access to namespace domains.
pub struct ObjectStore {
    domains: BTreeMap<String, Arc<Mutex<RamDomain>>>,
}

impl ObjectStore {
    pub fn new() -> Self {
        Self {
            domains: BTreeMap::new(),
        }
    }

    /// Fetches an existing domain, or generates a new one.
    pub fn get_domain(&mut self, name: &str) -> Arc<Mutex<RamDomain>> {
        if let Some(domain) = self.domains.get(name) {
            domain.clone()
        } else {
            let new_domain = Arc::new(Mutex::new(RamDomain::new()));
            self.domains.insert(String::from(name), new_domain.clone());
            new_domain.clone()
        }
    }

    /// Retrieves a list of all current Domain Names.
    pub fn list_domains(&self) -> Vec<String> {
        self.domains.keys().cloned().collect()
    }
}

/// A thread-safe global instance of the ObjectStore.
pub static STORE: Lazy<Mutex<ObjectStore>> = Lazy::new(|| Mutex::new(ObjectStore::new()));
