//! SQL identifiers: quoting, splitting a qualified name, and comparing a word
//! with PostgreSQL's case folding. Every place that writes or reads a name uses
//! these.

use pgrx::pg_sys;

/// `name` double-quoted, internal quotes doubled: safe anywhere, the form for SQL
/// `pg_tviews` writes.
#[must_use]
pub fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `name` quoted only where SQL needs it, as PostgreSQL's `quote_ident()` writes
/// it: unchanged when it is a plain lower-case identifier that is no keyword. The
/// form to use where text is compared with what PostgreSQL itself prints
/// (`pg_get_viewdef`, `regclass` output).
#[must_use]
pub fn quote_if_needed(name: &str) -> String {
    let Ok(c) = std::ffi::CString::new(name) else {
        return quoted(name);
    };
    // SAFETY: a NUL-terminated string; the result is copied before `c` drops.
    unsafe {
        std::ffi::CStr::from_ptr(pg_sys::quote_identifier(c.as_ptr()))
            .to_string_lossy()
            .into_owned()
    }
}

/// The dot-separated parts of `name`, double-quoted parts unquoted and taken as
/// written otherwise, or `None` if a part is empty or a quote is not closed.
#[must_use]
pub fn split(name: &str) -> Option<Vec<String>> {
    let mut parts = Vec::new();
    let mut chars = name.chars().peekable();
    loop {
        let mut part = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next()? {
                    '"' if chars.peek() == Some(&'"') => {
                        chars.next();
                        part.push('"');
                    }
                    '"' => break,
                    c => part.push(c),
                }
            }
            if !matches!(chars.peek(), None | Some('.')) {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' {
                    break;
                }
                part.push(c);
                chars.next();
            }
        }
        if part.is_empty() {
            return None;
        }
        parts.push(part);
        if chars.next().is_none() {
            return Some(parts);
        }
    }
}

/// Whether a word of SQL text names `name`, with PostgreSQL's folding: a quoted
/// word compares exactly, an unquoted one in lower case.
#[must_use]
pub fn names(word: &str, quoted: bool, name: &str) -> bool {
    if quoted {
        word == name
    } else {
        word.to_lowercase() == name
    }
}

#[cfg(test)]
mod tests {
    use super::{names, quoted, split};

    #[test]
    fn quoted_doubles_quotes() {
        assert_eq!(quoted("tv_post"), "\"tv_post\"");
        assert_eq!(quoted("a\"b"), "\"a\"\"b\"");
    }

    #[test]
    fn split_unquotes_and_refuses_malformed_names() {
        assert_eq!(
            split("app.tv_post"),
            Some(vec!["app".into(), "tv_post".into()])
        );
        assert_eq!(
            split("\"Odd.Schema\".tv_post"),
            Some(vec!["Odd.Schema".into(), "tv_post".into()])
        );
        assert_eq!(split("\"a\"\"b\""), Some(vec!["a\"b".into()]));
        for bad in ["", "app..tv_post", "\"app.tv_post", "\"app\"x.tv_post"] {
            assert_eq!(split(bad), None, "{bad}");
        }
    }

    #[test]
    fn names_folds_unquoted_words() {
        assert!(names("TV_POST", false, "tv_post"));
        assert!(!names("TV_POST", true, "tv_post"));
        assert!(names("TV_POST", true, "TV_POST"));
    }
}
