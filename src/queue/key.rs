use std::hash::{Hash, Hasher};

/// Identifies a unique TVIEW row to refresh: (entity, pk)
///
/// Example: RefreshKey { entity: "user".to_string(), pk: 42 }
/// represents the row in tv_user with pk_user = 42
#[derive(Debug, Clone, Eq, serde::Serialize, serde::Deserialize)]
pub struct RefreshKey {
    /// Entity name (e.g., "user", "post", "company")
    pub entity: String,

    /// Primary key value (pk_<entity>)
    pub pk: i64,
}

impl PartialEq for RefreshKey {
    fn eq(&self, other: &Self) -> bool {
        self.entity == other.entity && self.pk == other.pk
    }
}

impl Hash for RefreshKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.entity.hash(state);
        self.pk.hash(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_refresh_key_equality() {
        let key1 = RefreshKey { entity: "user".to_string(), pk: 42 };
        let key2 = RefreshKey { entity: "user".to_string(), pk: 42 };
        let key3 = RefreshKey { entity: "user".to_string(), pk: 43 };

        assert_eq!(key1, key2);
        assert_ne!(key1, key3);
    }

    #[test]
    fn test_refresh_key_hashset_dedup() {
        let mut set = std::collections::HashSet::new();

        set.insert(RefreshKey { entity: "user".to_string(), pk: 42 });
        set.insert(RefreshKey { entity: "user".to_string(), pk: 42 }); // duplicate
        set.insert(RefreshKey { entity: "post".to_string(), pk: 42 });

        assert_eq!(set.len(), 2); // Only 2 unique keys
    }
}