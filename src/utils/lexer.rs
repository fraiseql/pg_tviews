//! SQL text read by PostgreSQL's own lexer (`core_yylex`, as PL/pgSQL reads it):
//! comments, quoted names, string and dollar-quoted literals, and the server's
//! `standard_conforming_strings` are read exactly as the server reads them.

use pgrx::pg_sys;
use std::ffi::{CStr, CString};

/// A token of SQL text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Token {
    /// Byte offset of its first character in the text.
    pub at: usize,
    pub kind: Kind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    /// A keyword, in lower case.
    Keyword(String),
    /// An identifier, as PostgreSQL names it: folded when unquoted, as written
    /// when quoted.
    Ident(String),
    /// A one-character token: `(`, `)`, `.`, `;`, …
    Char(char),
    /// Anything else: a literal, an operator, a parameter.
    Other,
}

impl Token {
    /// Whether it is keyword `word` (lower case).
    #[must_use]
    pub fn is_keyword(&self, word: &str) -> bool {
        matches!(&self.kind, Kind::Keyword(k) if k == word)
    }

    /// The name it gives, when it is a word: an identifier, or a keyword used as one.
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        match &self.kind {
            Kind::Keyword(k) | Kind::Ident(k) => Some(k),
            _ => None,
        }
    }
}

/// The token code our keyword table gives every keyword: above any code the
/// grammar uses, so a keyword is told apart without the grammar's token numbers.
const KEYWORD: u16 = u16::MAX;

/// The tokens of `sql`, or `None` if it holds a NUL byte. A lexical error (an
/// unterminated literal) raises, as the server's own parse of the text would.
#[must_use]
pub fn tokens(sql: &str) -> Option<Vec<Token>> {
    let text = CString::new(sql).ok()?;
    // SAFETY: `scanner_init` copies `text` into a buffer of the current memory
    // context and reads the keyword list the server exports; `keyword_tokens` has
    // one code per keyword of that list and outlives the scanner, which is
    // finished before returning. `keyword` and `str_` point into memory the
    // scanner allocated, read before the next call.
    unsafe {
        let count = usize::try_from(pg_sys::ScanKeywords.num_keywords).unwrap_or(0);
        let keyword_tokens = vec![KEYWORD; count];
        let ident = lex(c"pg_tviews_ident", &keyword_tokens, None)
            .first()
            .map(|(code, _, _)| *code);
        Some(
            lex(&text, &keyword_tokens, ident)
                .into_iter()
                .map(|(code, at, value)| Token {
                    at,
                    kind: match (code, value) {
                        (c, Some(v)) if c == i32::from(KEYWORD) => Kind::Keyword(v),
                        (c, Some(v)) if Some(c) == ident => Kind::Ident(v),
                        (c, _) if (1..128).contains(&c) => {
                            Kind::Char(char::from(u8::try_from(c).unwrap_or(b'?')))
                        }
                        _ => Kind::Other,
                    },
                })
                .collect(),
        )
    }
}

/// `(token code, byte offset, name)` of each token of `text`; the name of a
/// keyword, and of an identifier when `ident` is the identifier's code.
///
/// SAFETY: `keyword_tokens` has `ScanKeywords.num_keywords` entries.
unsafe fn lex(
    text: &CStr,
    keyword_tokens: &[u16],
    ident: Option<i32>,
) -> Vec<(i32, usize, Option<String>)> {
    let mut found = Vec::new();
    // SAFETY: see `tokens`.
    unsafe {
        let mut extra: pg_sys::core_yy_extra_type = std::mem::zeroed();
        let scanner = pg_sys::scanner_init(
            text.as_ptr(),
            &raw mut extra,
            &raw const pg_sys::ScanKeywords,
            keyword_tokens.as_ptr(),
        );
        loop {
            let mut value = pg_sys::core_YYSTYPE::default();
            let mut at: std::ffi::c_int = 0;
            let code = pg_sys::core_yylex(&raw mut value, &raw mut at, scanner);
            if code == 0 {
                break;
            }
            // The scanner sets `keyword` for a keyword and `str_` for an
            // identifier; other tokens set another member, or none.
            let string = if code == i32::from(KEYWORD) {
                value.keyword
            } else if Some(code) == ident {
                value.str_.cast_const()
            } else {
                std::ptr::null()
            };
            found.push((
                code,
                usize::try_from(at).unwrap_or(0),
                (!string.is_null()).then(|| CStr::from_ptr(string).to_string_lossy().into_owned()),
            ));
        }
        pg_sys::scanner_finish(scanner);
    }
    found
}
