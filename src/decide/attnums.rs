//! A set of attribute numbers.

/// A set of a table's attribute numbers (positive), as a bitset: a table has at
/// most 1,600 columns.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Attnums {
    words: Vec<u64>,
}

impl Attnums {
    /// The empty set.
    #[must_use]
    pub const fn new() -> Self {
        Self { words: Vec::new() }
    }

    /// Add `attnum`; one that is not positive names no column and is ignored.
    pub fn insert(&mut self, attnum: i16) {
        let Some((word, bit)) = slot(attnum) else {
            return;
        };
        if self.words.len() <= word {
            self.words.resize(word + 1, 0);
        }
        self.words[word] |= 1 << bit;
    }

    /// Whether `attnum` is in the set.
    #[must_use]
    pub fn contains(&self, attnum: i16) -> bool {
        slot(attnum)
            .is_some_and(|(word, bit)| self.words.get(word).is_some_and(|w| w & (1 << bit) != 0))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.words.iter().all(|w| *w == 0)
    }

    /// Whether no attnum is in both sets.
    #[must_use]
    pub fn is_disjoint(&self, other: &Self) -> bool {
        self.words.iter().zip(&other.words).all(|(a, b)| a & b == 0)
    }

    /// The attnums in both sets.
    #[must_use]
    pub fn intersection(&self, other: &Self) -> Self {
        let mut words: Vec<u64> = self
            .words
            .iter()
            .zip(&other.words)
            .map(|(a, b)| a & b)
            .collect();
        while words.last() == Some(&0) {
            words.pop();
        }
        Self { words }
    }

    /// The attnums, ascending.
    pub fn iter(&self) -> impl Iterator<Item = i16> + '_ {
        self.words.iter().enumerate().flat_map(|(i, &w)| {
            (0..64_u16)
                .filter(move |b| w & (1 << b) != 0)
                .filter_map(move |b| i16::try_from(i * 64 + usize::from(b) + 1).ok())
        })
    }
}

/// The word and bit of a positive attnum.
fn slot(attnum: i16) -> Option<(usize, u32)> {
    let n = usize::try_from(attnum).ok()?.checked_sub(1)?;
    Some((n / 64, u32::try_from(n % 64).ok()?))
}

impl FromIterator<i16> for Attnums {
    fn from_iter<I: IntoIterator<Item = i16>>(iter: I) -> Self {
        let mut set = Self::new();
        for attnum in iter {
            set.insert(attnum);
        }
        set
    }
}

impl<'a> FromIterator<&'a i16> for Attnums {
    fn from_iter<I: IntoIterator<Item = &'a i16>>(iter: I) -> Self {
        iter.into_iter().copied().collect()
    }
}

impl std::fmt::Debug for Attnums {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_set().entries(self.iter()).finish()
    }
}
