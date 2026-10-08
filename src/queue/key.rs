/// The value of a TVIEW's identity column for one row (ADR 0169).
///
/// `Int` for an integer identity (`int2`, `int4`, `int8`: the trinity `pk_*` hot
/// path, no text round trip); `Text` for any other type, in its canonical output
/// text, cast back to the column's type where it is bound.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum KeyValue {
    Int(i64),
    Text(String),
}

impl KeyValue {
    /// The integer value of an `Int` key.
    #[must_use]
    pub const fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Text(_) => None,
        }
    }

    /// The key as an integer: an `Int` key, or a `Text` key that spells one.
    #[must_use]
    pub fn to_int(&self) -> Option<i64> {
        match self {
            Self::Int(v) => Some(*v),
            Self::Text(t) => t.parse().ok(),
        }
    }
}

impl std::fmt::Display for KeyValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Int(v) => write!(f, "{v}"),
            Self::Text(v) => f.write_str(v),
        }
    }
}

/// Identifies a unique TVIEW row to refresh: an entity and the value of its
/// identity column, or every row of the entity.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct RefreshKey {
    /// Entity name (e.g., "user", "post", "company")
    pub entity: String,

    /// The row's identity value.
    pub key: KeyValue,

    /// Every row of the entity's TVIEW (issues #157, #158): a write to a base table
    /// no cascade maps, under the `full_refresh` policy. `key` is `Int(0)`.
    pub all: bool,
}

impl RefreshKey {
    /// A key of an integer identity.
    pub fn pk(entity: impl Into<String>, pk: i64) -> Self {
        Self::new(entity, KeyValue::Int(pk))
    }

    /// A key of any identity.
    pub fn new(entity: impl Into<String>, key: KeyValue) -> Self {
        Self {
            entity: entity.into(),
            key,
            all: false,
        }
    }

    /// Construct a key for every row of the entity's TVIEW.
    pub fn all(entity: impl Into<String>) -> Self {
        Self {
            entity: entity.into(),
            key: KeyValue::Int(0),
            all: true,
        }
    }

    /// Returns `true` if this key stands for every row of the TVIEW.
    #[must_use]
    pub const fn is_all(&self) -> bool {
        self.all
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
    fn int_and_text_keys_differ_and_deduplicate() {
        let mut set = std::collections::HashSet::new();
        set.insert(RefreshKey::new("doc", KeyValue::Text("42".into())));
        set.insert(RefreshKey::new("doc", KeyValue::Text("42".into())));
        set.insert(RefreshKey::pk("doc", 42));
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn test_all_key_is_distinct_and_deduplicated() {
        let mut set = std::collections::HashSet::new();
        set.insert(RefreshKey::all("order"));
        set.insert(RefreshKey::all("order"));
        set.insert(RefreshKey::pk("order", 0));
        assert_eq!(set.len(), 2);
        assert!(RefreshKey::all("order").is_all());
        assert!(!RefreshKey::pk("order", 0).is_all());
    }

    #[test]
    fn key_value_text_and_int() {
        assert_eq!(KeyValue::Int(7).as_int(), Some(7));
        assert_eq!(KeyValue::Text("a".into()).as_int(), None);
        assert_eq!(KeyValue::Int(-3).to_string(), "-3");
        assert_eq!(
            KeyValue::Text("00000000-0000-0000-0000-000000000001".into()).to_string(),
            "00000000-0000-0000-0000-000000000001"
        );
    }
}
