use alloc::string::String;
use alloc::vec::Vec;
use alloc::collections::BTreeMap;

/// An in-memory storage domain containing object variables.
pub struct RamDomain {
    objects: BTreeMap<String, Vec<u8>>,
}

impl RamDomain {
    pub fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }

    /// Sets or overrides a key-value pair in this domain.
    pub fn set(&mut self, key: &str, data: Vec<u8>) {
        self.objects.insert(String::from(key), data);
    }

    /// Retrieves a copy of the data associated with the key, if any.
    pub fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.objects.get(key).cloned()
    }

    /// Deletes a key from the domain. Returns true if it existed.
    pub fn delete(&mut self, key: &str) -> bool {
        self.objects.remove(key).is_some()
    }

    /// Lists all keys currently in the domain.
    pub fn list(&self) -> Vec<String> {
        self.objects.keys().cloned().collect()
    }
}
