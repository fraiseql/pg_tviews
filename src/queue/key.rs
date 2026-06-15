use std::hash::{Hash, Hasher};

/// Identifies a unique TVIEW row to refresh.
///
/// For standard TVIEWs the key is a `(entity, pk)` pair; for DISTINCT ON TVIEWs
/// the key is a `(entity, dedup_key)` pair where `dedup_key` is the value of the
/// DISTINCT ON column stored as TEXT.
#[derive(Debug, Clone, Eq, serde::Serialize, serde::Deserialize)]
pub struct RefreshKey {
    /// Entity name (e.g., "user", "post", "company")
    pub entity: String,

    /// Primary key value (pk_<entity>).  Used for standard TVIEWs.
    /// Set to 0 when `dedup_key` is present.
    pub pk: i64,

    /// Deduplication key for DISTINCT ON TVIEWs (stored as TEXT).
    /// When `Some`, the key identifies the DISTINCT ON group to re-evaluate.
    /// When `None`, `pk` is the authoritative identifier.
    #[serde(default)]
    pub dedup_key: Option<String>,
}

impl RefreshKey {
    /// Construct a standard PK-based key.
    pub fn pk(entity: impl Into<String>, pk: i64) -> Self {
        Self {
            entity: entity.into(),
            pk,
            dedup_key: None,
        }
    }

    /// Construct a DISTINCT ON dedup key.
    pub fn dedup(entity: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            pk: 0,
            dedup_key: Some(key.into()),
        }
    }

    /// Returns `true` if this is a DISTINCT ON dedup key.
    #[must_use]
    pub fn is_dedup(&self) -> bool {
        self.dedup_key.is_some()
    }
}

impl PartialEq for RefreshKey {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity && self.pk == other.pk && self.dedup_key == other.dedup_key
    }
}

impl Hash for RefreshKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity.hash(state);
        self.pk.hash(state);
        self.dedup_key.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_key_equality() {
        let key1 = RefreshKey::pk("user", 42);
        let key2 = RefreshKey::pk("user", 42);
        let key3 = RefreshKey::pk("user", 43);

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_refresh_key_hashset_dedup() {
        let mut set = std::collections::HashSet::new();

        set.insert(RefreshKey::pk("user", 42));
        set.insert(RefreshKey::pk("user", 42)); // duplicate
        set.insert(RefreshKey::pk("post", 42));

        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_dedup_key_distinct_from_pk_key() {
        let pk_key = RefreshKey::pk("contract", 42);
        let dedup_key = RefreshKey::dedup("contract", "some-uuid");
        assert_ne!(pk_key, dedup_key);
    }

    #[test]
    fn test_dedup_key_hashset() {
        let mut set = std::collections::HashSet::new();
        set.insert(RefreshKey::dedup("contract", "uuid-1"));
        set.insert(RefreshKey::dedup("contract", "uuid-1")); // duplicate
        set.insert(RefreshKey::dedup("contract", "uuid-2"));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_is_dedup() {
        assert!(!RefreshKey::pk("user", 1).is_dedup());
        assert!(RefreshKey::dedup("contract", "uuid").is_dedup());
    }

    #[test]
    fn test_serde_roundtrip_pk() {
        let key = RefreshKey::pk("user", 99);
        let json = serde_json::to_string(&key).unwrap();
        let back: RefreshKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn test_serde_roundtrip_dedup() {
        let key = RefreshKey::dedup("contract", "some-uuid-val");
        let json = serde_json::to_string(&key).unwrap();
        let back: RefreshKey = serde_json::from_str(&json).unwrap();
        assert_eq!(key, back);
    }

    #[test]
    fn test_serde_backward_compat_no_dedup_key_field() {
        // Old serialized format has no "dedup_key" field — must default to None
        let json = r#"{"entity":"user","pk":42}"#;
        let key: RefreshKey = serde_json::from_str(json).unwrap();
        assert_eq!(key.entity, "user");
        assert_eq!(key.pk, 42);
        assert!(key.dedup_key.is_none());
    }
}
