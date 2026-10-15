//! How a TVIEW's table is stored: persistence, fillfactor, the GIN index on
//! `data`, and how `data` itself is compressed and kept (#225).

use crate::error::{TViewError, TViewResult};

/// The compression of `data` (`ALTER TABLE … ALTER COLUMN data SET COMPRESSION`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Compression {
    Pglz,
    Lz4,
}

/// The storage strategy of `data` (`ALTER TABLE … ALTER COLUMN data SET
/// STORAGE`). `PLAIN` is not one: `jsonb` is a varlena that must be toastable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataStorage {
    /// Compressed, then moved out of line when still too large.
    Extended,
    /// Compressed and kept inline, out of line only as a last resort.
    Main,
    /// Out of line, never compressed.
    External,
}

impl Compression {
    pub const NAMES: &'static str = "pglz or lz4";

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "pglz" => Some(Self::Pglz),
            "lz4" => Some(Self::Lz4),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Pglz => "pglz",
            Self::Lz4 => "lz4",
        }
    }

    /// Whether this server was built with it (lz4 is a build option).
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn available(self) -> TViewResult<bool> {
        Ok(crate::utils::spi::one::<bool>(
            "SELECT $1 = ANY (enumvals) FROM pg_catalog.pg_settings \
             WHERE name = 'default_toast_compression'",
            &[crate::utils::spi::text(self.name())],
        )?
        .unwrap_or(false))
    }

    /// Fail unless this server can compress with it: a server built without lz4
    /// would refuse it with a message that does not name the option.
    ///
    /// # Errors
    /// [`TViewError::InvalidInput`] naming the option, or the catalog cannot be
    /// read.
    pub fn check_available(self) -> TViewResult<()> {
        if self.available()? {
            Ok(())
        } else {
            Err(TViewError::InvalidInput {
                parameter: "data_compression".to_string(),
                reason: format!(
                    "{} is not available: this PostgreSQL server was built without it; \
                     declare \"pglz\"",
                    self.name()
                ),
            })
        }
    }

    /// A TVIEW's compression unless declared: lz4 where the server has it (it
    /// decompresses several times faster than pglz), pglz otherwise.
    ///
    /// # Errors
    /// The catalog cannot be read.
    pub fn server_default() -> TViewResult<Self> {
        Ok(if Self::Lz4.available()? {
            Self::Lz4
        } else {
            Self::Pglz
        })
    }
}

impl DataStorage {
    pub const NAMES: &'static str = "extended, main or external";

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "extended" => Some(Self::Extended),
            "main" => Some(Self::Main),
            "external" => Some(Self::External),
            _ => None,
        }
    }

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Extended => "extended",
            Self::Main => "main",
            Self::External => "external",
        }
    }
}

/// The range `toast_tuple_target` takes (PostgreSQL's, for 8 kB pages).
pub const TOAST_TUPLE_TARGET: std::ops::RangeInclusive<i32> = 128..=8160;

/// Storage of a TVIEW's table: its persistence, its fillfactor, whether `data`
/// has a GIN index, and how `data` is compressed and kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Storage {
    pub logged: bool,
    pub fillfactor: i32,
    pub data_gin_index: bool,
    pub data_compression: Compression,
    pub data_storage: DataStorage,
    /// `None`: PostgreSQL's default.
    pub toast_tuple_target: Option<i32>,
}

impl Storage {
    /// What a TVIEW gets unless its options say otherwise (ADR 0220): LOGGED,
    /// fillfactor 85 (room for HOT updates), no GIN index on `data`, and `data`
    /// compressed and kept inline (MAIN), so a document is read back without a
    /// TOAST lookup. Its compression is [`Compression::server_default`], lz4 here.
    pub const DEFAULT: Self = Self {
        logged: true,
        fillfactor: 85,
        data_gin_index: false,
        data_compression: Compression::Lz4,
        data_storage: DataStorage::Main,
        toast_tuple_target: None,
    };

    /// The `data` column's clauses in `CREATE TABLE`.
    #[must_use]
    pub fn data_column_clause(&self) -> String {
        format!(
            " STORAGE {} COMPRESSION {}",
            self.data_storage.name().to_uppercase(),
            self.data_compression.name()
        )
    }

    /// `WITH (…)` for a new TVIEW table: fillfactor below 100 (the heap
    /// default) and a declared `toast_tuple_target`; empty when neither.
    #[must_use]
    pub fn with_clause(&self) -> String {
        let mut parameters = Vec::new();
        if self.fillfactor < 100 {
            parameters.push(format!("fillfactor = {}", self.fillfactor));
        }
        if let Some(target) = self.toast_tuple_target {
            parameters.push(format!("toast_tuple_target = {target}"));
        }
        if parameters.is_empty() {
            String::new()
        } else {
            format!(" WITH ({})", parameters.join(", "))
        }
    }

    /// Whether the stored values must be written again to take `self`:
    /// PostgreSQL applies a compression or storage change to new values only.
    #[must_use]
    pub fn rewrites_rows_from(&self, current: &Self) -> bool {
        self.data_compression != current.data_compression
            || self.data_storage != current.data_storage
            || self.toast_tuple_target != current.toast_tuple_target
    }
}
