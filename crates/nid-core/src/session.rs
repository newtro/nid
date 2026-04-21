//! Session identifiers.

use serde::{Deserialize, Serialize};

/// Opaque short identifier for a persisted session. Format: `sess_<10 hex>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub String);

impl SessionId {
    pub fn new_random() -> Self {
        use rand::RngCore;
        let mut bytes = [0u8; 5];
        rand::thread_rng().fill_bytes(&mut bytes);
        SessionId(format!("sess_{}", hex::encode(bytes)))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for SessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Handle handed back in CompressionResult pointing at a persisted raw blob.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionRef(pub String);

impl SessionRef {
    pub fn new(id: String) -> Self {
        SessionRef(id)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_id_has_prefix() {
        let id = SessionId::new_random();
        assert!(id.as_str().starts_with("sess_"));
        assert_eq!(id.as_str().len(), "sess_".len() + 10);
    }

    #[test]
    fn session_ids_are_unique_enough() {
        let a = SessionId::new_random();
        let b = SessionId::new_random();
        assert_ne!(a, b);
    }
}
