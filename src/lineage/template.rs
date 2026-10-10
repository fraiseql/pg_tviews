//! SQL templates: generated SQL with relations and columns named by OID.

/// A piece of generated SQL: text, or a column of a table occurrence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Piece {
    Text(String),
    Column { occ: usize, attnum: i16 },
}

/// SQL with holes for occurrence columns, filled in when a query is assembled.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Sql(pub Vec<Piece>);

impl Sql {
    pub fn text(s: impl Into<String>) -> Self {
        Self(vec![Piece::Text(s.into())])
    }

    pub fn push_text(&mut self, s: &str) {
        if let Some(Piece::Text(last)) = self.0.last_mut() {
            last.push_str(s);
        } else {
            self.0.push(Piece::Text(s.to_string()));
        }
    }

    pub fn push_sql(&mut self, other: Self) {
        for piece in other.0 {
            match piece {
                Piece::Text(t) => self.push_text(&t),
                column @ Piece::Column { .. } => self.0.push(column),
            }
        }
    }
}

/// A stored mapping query is a template that names relations and columns by OID
/// and attribute number, `{r:<relid>}` and `{c:<relid>:<attnum>}`, so that renames
/// leave it valid; `{` and `}` of the SQL itself are doubled. [`render_template`]
/// writes the current names in.
#[must_use]
pub fn escape_template(text: &str) -> String {
    text.replace('{', "{{").replace('}', "}}")
}

/// A placeholder of a mapping-query template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Placeholder {
    Relation(u32),
    Column(u32, i16),
}

/// Fill in a template with `name(placeholder)`; `None` if a placeholder is
/// malformed or `name` has no name for it (a dropped relation or column).
pub fn fill_template(
    template: &str,
    name: &dyn Fn(Placeholder) -> Option<String>,
) -> Option<String> {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(i) = rest.find(['{', '}']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if tail.starts_with("{{") || tail.starts_with("}}") {
            out.push_str(&tail[..1]);
            rest = &tail[2..];
            continue;
        }
        let end = tail.find('}')?;
        let mut fields = tail[1..end].split(':');
        let placeholder = match (fields.next()?, fields.next(), fields.next(), fields.next()) {
            ("r", Some(relid), None, None) => Placeholder::Relation(relid.parse().ok()?),
            ("c", Some(relid), Some(attnum), None) => {
                Placeholder::Column(relid.parse().ok()?, attnum.parse().ok()?)
            }
            _ => return None,
        };
        out.push_str(&name(placeholder)?);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The placeholders a template uses.
#[must_use]
pub fn template_placeholders(template: &str) -> Vec<Placeholder> {
    let found = std::cell::RefCell::new(Vec::new());
    let _ = fill_template(template, &|p| {
        found.borrow_mut().push(p);
        Some(String::new())
    });
    found.into_inner()
}

/// The relation a mapping query reads the changed rows from: the old and new
/// images of the statement's rows (a CTE over the transition tables), or a table of
/// that name in tests.
pub const DELTA: &str = "pg_tviews_delta";

/// A mapping-query template with the current names of its relations and columns;
/// `None` when one of them is gone.
///
/// # Errors
/// Returns an error if the catalog cannot be read.
pub fn render_template(template: &str) -> crate::TViewResult<Option<String>> {
    use pgrx::prelude::*;
    let mut names: std::collections::HashMap<(u32, i16), Option<String>> =
        std::collections::HashMap::new();
    for placeholder in template_placeholders(template) {
        let (relid, attnum) = match placeholder {
            Placeholder::Relation(relid) => (relid, 0),
            Placeholder::Column(relid, attnum) => (relid, attnum),
        };
        if names.contains_key(&(relid, attnum)) {
            continue;
        }
        let args = [
            crate::utils::spi::oid(pgrx::pg_sys::Oid::from(relid)),
            crate::utils::spi::int2(attnum),
        ];
        let name = Spi::get_one_with_args::<String>(
            "SELECT CASE WHEN $2 = 0 \
                    THEN pg_catalog.quote_ident(n.nspname) || '.' || pg_catalog.quote_ident(c.relname) \
                    ELSE (SELECT pg_catalog.quote_ident(a.attname) FROM pg_catalog.pg_attribute a \
                          WHERE a.attrelid = c.oid AND a.attnum = $2 AND NOT a.attisdropped) END \
             FROM pg_catalog.pg_class c \
             JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
             WHERE c.oid = $1",
            &args,
        )?;
        names.insert((relid, attnum), name);
    }
    Ok(fill_template(template, &|p| {
        let key = match p {
            Placeholder::Relation(relid) => (relid, 0),
            Placeholder::Column(relid, attnum) => (relid, attnum),
        };
        names.get(&key).cloned().flatten()
    }))
}
