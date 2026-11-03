use crate::catalog::KeyType;
use pgrx::datum::DatumWithOid;

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

    /// Every row of the entity's TVIEW: a write to a base table
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

// ── Keys bound as parameters ───────────────────────────────────────────────

/// `param` (`$1`) cast to the identity's type, or to an array of it: an integer
/// identity is bound as `int8`, any other as text cast to its type, so a filter on
/// the identity column can use its index.
pub fn key_cast(key_type: &KeyType, param: &str, array: bool) -> String {
    let brackets = if array { "[]" } else { "" };
    match key_type {
        KeyType::Int => format!("{param}::pg_catalog.int8{brackets}"),
        KeyType::Text(ty) => format!("{param}::pg_catalog.text{brackets}::{ty}{brackets}"),
    }
}

/// The values of `keys` as the identity binds them; a text key of an integer
/// identity that is not an integer is an error.
fn key_values(key_type: &KeyType, keys: &[KeyValue]) -> crate::TViewResult<KeyValues> {
    Ok(match key_type {
        KeyType::Int => KeyValues::Int(
            keys.iter()
                .map(|k| {
                    k.to_int().ok_or_else(|| crate::TViewError::InvalidInput {
                        parameter: "key".to_string(),
                        reason: format!("{k} is not a value of an integer identity"),
                    })
                })
                .collect::<crate::TViewResult<_>>()?,
        ),
        KeyType::Text(_) => KeyValues::Text(keys.iter().map(ToString::to_string).collect()),
    })
}

enum KeyValues {
    Int(Vec<i64>),
    Text(Vec<String>),
}

/// `keys` as one array parameter, for `key_cast(…, true)`.
pub fn key_array(
    key_type: &KeyType,
    keys: &[KeyValue],
) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_values(key_type, keys)? {
        KeyValues::Int(v) => crate::utils::spi::int8_array(v),
        KeyValues::Text(v) => crate::utils::spi::text_array(v),
    })
}

/// The identities an upsert's source produced ([`Produced::keys`], as text), as
/// one array parameter for `key_cast(…, true)`.
pub fn produced_keys(
    key_type: &KeyType,
    keys: Vec<String>,
) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_type {
        KeyType::Int => crate::utils::spi::int8_array(
            keys.iter()
                .map(|k| {
                    k.parse().map_err(|_| crate::TViewError::InvalidInput {
                        parameter: "key".to_string(),
                        reason: format!("{k} is not a value of an integer identity"),
                    })
                })
                .collect::<crate::TViewResult<_>>()?,
        ),
        KeyType::Text(_) => crate::utils::spi::text_array(keys),
    })
}

/// One key as a parameter, for `key_cast(…, false)`.
pub fn key_scalar(key_type: &KeyType, key: &KeyValue) -> crate::TViewResult<DatumWithOid<'static>> {
    Ok(match key_values(key_type, std::slice::from_ref(key))? {
        KeyValues::Int(v) => crate::utils::spi::int8(v[0]),
        KeyValues::Text(mut v) => crate::utils::spi::text(v.swap_remove(0)),
    })
}

/// Identity keys bound from `$1` for a lookup, shaped by their count as
/// [`crate::utils::spi::LookupKeys`] shapes keys of `fk_<entity>`: a backing view
/// read by identity may join many rows of a table for each key (an aggregate of
/// children), which a generic plan for an array estimates as ten keys' worth.
pub struct KeyLookup {
    /// What the identity is compared with by `OPERATOR(pg_catalog.=)`.
    pub rhs: String,
    /// The keys, as `rhs` reads them.
    pub args: Vec<DatumWithOid<'static>>,
    /// How a statement looking them up is planned.
    pub plan: crate::utils::spi::Plan,
}

/// The lookup of `keys` of an identity of `key_type`; `None` without any.
///
/// # Errors
/// A text key of an integer identity that is not an integer.
pub fn key_lookup(key_type: &KeyType, keys: &[KeyValue]) -> crate::TViewResult<Option<KeyLookup>> {
    use crate::utils::spi::{Plan, padded, shape, shaped_rhs};
    if keys.is_empty() {
        return Ok(None);
    }
    Ok(Some(match shape(keys.len()) {
        Some(width) => KeyLookup {
            rhs: shaped_rhs(width, 1, |p| key_cast(key_type, p, false)),
            args: padded(keys.to_vec(), width)
                .iter()
                .map(|key| key_scalar(key_type, key))
                .collect::<crate::TViewResult<_>>()?,
            plan: Plan::Generic,
        },
        None => KeyLookup {
            rhs: format!("ANY ({})", key_cast(key_type, "$1", true)),
            args: vec![key_array(key_type, keys)?],
            plan: Plan::PerExecution,
        },
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_key_is_looked_up_by_equality() {
        let lookup = key_lookup(&KeyType::Int, &[KeyValue::Int(7)])
            .unwrap()
            .unwrap();
        assert_eq!(lookup.rhs, "$1::pg_catalog.int8");
        assert_eq!(lookup.args.len(), 1);
        let two = key_lookup(&KeyType::Int, &[KeyValue::Int(7), KeyValue::Int(8)])
            .unwrap()
            .unwrap();
        assert!(two.rhs.starts_with("ANY (ARRAY["), "{}", two.rhs);
    }

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
