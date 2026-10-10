//! `CREATE TABLE tv_* AS`: read from the parse tree and turned into a TVIEW.

use super::{
    CStr, Intercept, PgLogLevel, PgSqlErrorCode, Spi, TViewError, TViewResult, function_name,
    pg_guard, pg_sys,
};

/// A `CREATE [UNLOGGED] TABLE [IF NOT EXISTS] [schema.]tv_* [WITH (fillfactor = n)]
/// AS SELECT …`, read from the parse tree.
pub(super) struct Ctas {
    pub(super) target: CtasTarget,
    pub(super) query: String,
    pub(super) logged: Option<bool>,
    pub(super) fillfactor: Option<i32>,
}

/// The table a `CREATE TABLE … AS` creates.
pub(super) struct CtasTarget {
    /// The schema named in the statement; `None`: `current_schema()`.
    pub(super) schema: Option<String>,
    pub(super) table: String,
    pub(super) if_not_exists: bool,
}

impl CtasTarget {
    /// The name `pg_tviews_create_or_replace()` takes: `tv_<entity>` or
    /// `"schema".tv_<entity>`.
    fn name(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{}.{}", crate::utils::quote_ident(schema), self.table),
            None => self.table.clone(),
        }
    }

    /// Whether `IF NOT EXISTS` applies: a relation of that name exists in the
    /// schema the table would be created in. `PostgreSQL` then skips the statement
    /// with a notice. Uses SPI: call it outside `catch_unwind`.
    fn skipped(&self) -> TViewResult<bool> {
        if !self.if_not_exists {
            return Ok(false);
        }
        let args = [
            crate::utils::spi::text(self.schema.as_deref()),
            crate::utils::spi::text(self.table.as_str()),
        ];
        Spi::connect(|client| {
            client
                .select(
                    "SELECT EXISTS (SELECT 1 FROM pg_catalog.pg_class c \
                     JOIN pg_catalog.pg_namespace n ON n.oid = c.relnamespace \
                     WHERE c.relname = $2 AND n.nspname = COALESCE($1, current_schema()))",
                    None,
                    &args,
                )?
                .first()
                .get_one::<bool>()
        })
        .map(|exists| exists == Some(true))
        .map_err(|e| TViewError::CatalogError {
            operation: format!("Look up {}", self.name()),
            pg_error: e.to_string(),
        })
    }
}

/// Where a refused `CREATE TABLE tv_* AS` sends the user.
pub(super) const CTAS_HINT: &str = "Create or change the TVIEW with \
    SELECT tviews.pg_tviews_create_or_replace('tv_<entity>', $$<query>$$, \
    options => '{\"logged\": …, \"fillfactor\": …}').";

/// Read a `CREATE TABLE tv_* AS` into a decision, from the parse tree only (no
/// SPI: this runs inside `catch_unwind`).
///
/// Anything but a `tv_*` table target passes through. What a TVIEW cannot honour is
/// refused; the rest becomes a [`Ctas`] to create after `catch_unwind`. Both carry
/// the target, so that `IF NOT EXISTS` on an existing relation is checked first.
///
/// SAFETY: the pointers come from the `ProcessUtility` hook and are null-checked.
pub(super) unsafe fn inspect_create_table_as(
    ctas: *mut pg_sys::CreateTableAsStmt,
    pstmt: *const pg_sys::PlannedStmt,
    query_string: *const ::std::os::raw::c_char,
) -> Result<Intercept, TViewError> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        let Some(table_name) = tview_ctas_target(ctas.cast()) else {
            return Ok(Intercept::PassThrough);
        };
        // TEST ONLY: simulate a hook that never saw this statement (see the
        // missed-interception check in the event trigger).
        if crate::config::test_skip_ctas_intercept() {
            return Ok(Intercept::PassThrough);
        }
        let ctas_ref = &*ctas;
        let into = &*ctas_ref.into;
        let rel = &*into.rel;

        let schema = (!rel.schemaname.is_null()).then(|| {
            CStr::from_ptr(rel.schemaname)
                .to_string_lossy()
                .into_owned()
        });
        let target = || CtasTarget {
            schema: schema.clone(),
            table: table_name.clone(),
            if_not_exists: ctas_ref.if_not_exists,
        };
        let refuse = |reason: &str| Ok(Intercept::Refuse(reason.to_string(), Some(target())));
        if ctas_ref.is_select_into {
            return Ok(Intercept::Refuse(
                format!("SELECT … INTO {table_name} cannot create a TVIEW"),
                None,
            ));
        }
        let persistence = rel.relpersistence.cast_unsigned();
        if persistence == pg_sys::RELPERSISTENCE_TEMP
            || schema
                .as_deref()
                .is_some_and(|schema| schema == "pg_temp" || schema.starts_with("pg_temp_"))
        {
            return refuse(&format!("{table_name} cannot be a temporary TVIEW"));
        }
        if !into.colNames.is_null() {
            return refuse(&format!(
                "{table_name} takes its column names from its query, not from a column list"
            ));
        }
        if !into.tableSpaceName.is_null() {
            return refuse(&format!(
                "TABLESPACE is not supported for TVIEW {table_name}"
            ));
        }
        if !into.accessMethod.is_null() {
            return refuse(&format!("USING is not supported for TVIEW {table_name}"));
        }
        if into.skipData {
            return refuse(&format!(
                "WITH NO DATA is not supported: TVIEW {table_name} is always populated"
            ));
        }
        if is_execute(ctas_ref.query) {
            return refuse(&format!(
                "CREATE TABLE {table_name} AS EXECUTE cannot create a TVIEW"
            ));
        }
        if !ctas_ref.query.is_null() && contains_param(ctas_ref.query, std::ptr::null_mut()) {
            return refuse(&format!(
                "a query with parameters (such as PL/pgSQL variables) cannot define TVIEW \
                 {table_name}"
            ));
        }
        let mut fillfactor = None;
        for i in 0..pg_sys::list_length(into.options) {
            let option = pg_sys::list_nth(into.options, i).cast::<pg_sys::DefElem>();
            if option.is_null() || (*option).defname.is_null() {
                continue;
            }
            let name = CStr::from_ptr((*option).defname).to_string_lossy();
            if name != "fillfactor" || !(*option).defnamespace.is_null() {
                return refuse(&format!(
                    "storage parameter {name} is not supported for TVIEW {table_name}; only \
                     fillfactor is"
                ));
            }
            match option_integer(option) {
                Some(value) if (10..=100).contains(&value) => fillfactor = Some(value),
                _ => {
                    return refuse(&format!(
                        "fillfactor for TVIEW {table_name} must be an integer from 10 to 100"
                    ));
                }
            }
        }

        let sql = if query_string.is_null() {
            ""
        } else {
            CStr::from_ptr(query_string).to_str().unwrap_or("")
        };
        // `query_string` is the whole simple-query batch; slice out just this
        // statement, then strip its `CREATE TABLE … AS` prefix.
        let stmt_sql = statement_text(sql, pstmt);
        let query = extract_ctas_select(stmt_sql, &table_name).ok_or_else(|| {
            TViewError::InvalidSelectStatement {
                sql: stmt_sql.to_string(),
                reason: format!("Could not find 'CREATE TABLE {table_name} AS' in query"),
            }
        })?;
        Ok(Intercept::CreateTview(Ctas {
            target: target(),
            query,
            logged: (persistence == pg_sys::RELPERSISTENCE_UNLOGGED).then_some(false),
            fillfactor,
        }))
    }
}

/// The `tv_*` table a `CREATE TABLE … AS` (or `SELECT … INTO`) creates, if `node`
/// is one. A `CREATE MATERIALIZED VIEW` is left to `PostgreSQL`.
///
/// SAFETY: `node` must be null or a valid `Node*`.
pub(super) unsafe fn tview_ctas_target(node: *mut pg_sys::Node) -> Option<String> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        if node.is_null() || (*node).type_ != pg_sys::NodeTag::T_CreateTableAsStmt {
            return None;
        }
        #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → CreateTableAsStmt* cast
        let ctas = &*node.cast::<pg_sys::CreateTableAsStmt>();
        if ctas.objtype != pg_sys::ObjectType::OBJECT_TABLE
            || ctas.into.is_null()
            || (*ctas.into).rel.is_null()
            || (*(*ctas.into).rel).relname.is_null()
        {
            return None;
        }
        let table = CStr::from_ptr((*(*ctas.into).rel).relname).to_str().ok()?;
        (table.starts_with("tv_") && table.len() > 3).then(|| table.to_string())
    }
}

/// The statement a utility `Query` wraps, as parse analysis leaves `EXECUTE` and
/// explained statements; any other node as it is.
///
/// SAFETY: `node` must be null or a valid `Node*`.
pub(super) unsafe fn utility_of(node: *mut pg_sys::Node) -> *mut pg_sys::Node {
    // SAFETY: `node` is checked for null before it is dereferenced.
    unsafe {
        if !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_Query {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Query* cast
            let utility = (*node.cast::<pg_sys::Query>()).utilityStmt;
            if !utility.is_null() {
                return utility;
            }
        }
        node
    }
}

/// Whether a `CREATE TABLE … AS` query is an `EXECUTE`, raw or analyzed.
///
/// SAFETY: `query` must be null or a valid `Node*`.
pub(super) unsafe fn is_execute(query: *mut pg_sys::Node) -> bool {
    // SAFETY: `utility_of` returns null or a valid node.
    unsafe {
        let node = utility_of(query);
        !node.is_null() && (*node).type_ == pg_sys::NodeTag::T_ExecuteStmt
    }
}

/// Whether an analyzed query or expression contains a `Param` node.
///
/// SAFETY: a `tree_walker` callback over a valid parse tree.
#[pg_guard]
pub(super) unsafe extern "C-unwind" fn contains_param(
    node: *mut pg_sys::Node,
    context: *mut std::ffi::c_void,
) -> bool {
    if node.is_null() {
        return false;
    }
    // SAFETY: `node` is a valid node of the tree being walked.
    unsafe {
        match (*node).type_ {
            pg_sys::NodeTag::T_Param => true,
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Query* cast
            pg_sys::NodeTag::T_Query => pg_sys::query_tree_walker(
                node.cast::<pg_sys::Query>(),
                Some(contains_param),
                context,
                0,
            ),
            _ => pg_sys::expression_tree_walker(node, Some(contains_param), context),
        }
    }
}

/// The integer value of a `WITH (name = value)` option, if it is one.
///
/// SAFETY: `option` must be null or a valid `DefElem*`.
pub(super) unsafe fn option_integer(option: *mut pg_sys::DefElem) -> Option<i32> {
    // SAFETY: every pointer is checked for null before it is dereferenced.
    unsafe {
        if option.is_null() || (*option).arg.is_null() {
            return None;
        }
        let arg = (*option).arg;
        match (*arg).type_ {
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → Integer* cast
            pg_sys::NodeTag::T_Integer => Some((*arg.cast::<pg_sys::Integer>()).ival),
            #[allow(clippy::cast_ptr_alignment)] // Reason: PostgreSQL Node* → String* cast
            pg_sys::NodeTag::T_String => {
                let value = (*arg.cast::<pg_sys::String>()).sval;
                (!value.is_null())
                    .then(|| CStr::from_ptr(value).to_str().ok()?.parse().ok())
                    .flatten()
            }
            _ => None,
        }
    }
}

/// Whether `target` is skipped by `IF NOT EXISTS`, so the statement goes to
/// `PostgreSQL`, which skips it with its notice. Raises if the lookup fails.
pub(super) fn skipped_or_raise(target: &CtasTarget) -> bool {
    target.skipped().unwrap_or_else(|e| e.raise())
}

/// Create the TVIEW a `CREATE TABLE tv_* AS` names, with `CREATE TABLE AS`
/// semantics, and report its rows in the command tag (`SELECT n`), as
/// `PostgreSQL` would. Runs outside `catch_unwind`: errors are raised as they are.
///
/// SAFETY: `qc` must be null or the hook's valid `QueryCompletion*`.
pub(super) unsafe fn create_tview_from_ctas(ctas: &Ctas, qc: *mut pg_sys::QueryCompletion) {
    if !crate::revision::is_current() {
        crate::revision::check();
    }
    let created = crate::ddl::replace::create_only(
        &ctas.target.name(),
        &ctas.query,
        crate::ddl::replace::Options::storage(ctas.logged, ctas.fillfactor),
        ctas.target.if_not_exists,
    );
    match created {
        Ok(crate::ddl::replace::Created::Rows(rows)) => {
            if !qc.is_null() {
                // SAFETY: `qc` is the hook's completion record.
                unsafe {
                    (*qc).commandTag = pg_sys::CommandTag::CMDTAG_SELECT;
                    (*qc).nprocessed = rows;
                }
            }
        }
        Ok(crate::ddl::replace::Created::Skipped) => {}
        Ok(crate::ddl::replace::Created::Exists(name)) => {
            pg_sys::panic::ErrorReport::new(
                PgSqlErrorCode::ERRCODE_DUPLICATE_TABLE,
                format!("TVIEW {name} already exists"),
                function_name!(),
            )
            .set_hint(CTAS_HINT)
            .report(PgLogLevel::ERROR);
        }
        Err(e) => e.raise(),
    }
}

/// The text of the single statement `pstmt` covers within a (possibly multi-statement)
/// `query_string`, using `stmt_location` / `stmt_len` (`-1` / `0` mean "unknown" and
/// "to the end of the string"). Falls back to the whole string if the range is invalid.
///
/// SAFETY: `pstmt` must be null or a valid `PlannedStmt*`.
pub(super) unsafe fn statement_text(query_string: &str, pstmt: *const pg_sys::PlannedStmt) -> &str {
    if pstmt.is_null() {
        return query_string;
    }
    // SAFETY: the caller's statement, checked for null above.
    let (location, len) = unsafe { ((*pstmt).stmt_location, (*pstmt).stmt_len) };
    let Ok(start) = usize::try_from(location) else {
        return query_string;
    };
    let end = match usize::try_from(len) {
        Ok(len) if len > 0 => start.saturating_add(len),
        _ => query_string.len(),
    };
    query_string
        .get(start..end.min(query_string.len()))
        .unwrap_or(query_string)
}

/// Extract the SELECT from the text of one `CREATE TABLE [schema.]tv_x AS SELECT …`
/// statement: everything after the `AS` that follows the table name, without a
/// trailing `;`. The statement is tokenized, so comments, quoted names and string
/// literals are read as PostgreSQL reads them.
pub(super) fn extract_ctas_select(stmt_sql: &str, table_name: &str) -> Option<String> {
    use sqlparser::dialect::PostgreSqlDialect;
    use sqlparser::keywords::Keyword;
    use sqlparser::tokenizer::{Token, Tokenizer};
    let tokens = Tokenizer::new(&PostgreSqlDialect {}, stmt_sql)
        .tokenize_with_location()
        .ok()?;
    let offsets = crate::ddl::rename::byte_offsets(stmt_sql, &tokens);
    let sig: Vec<usize> = (0..tokens.len())
        .filter(|&i| !matches!(tokens[i].token, Token::Whitespace(_)))
        .collect();
    let word = |i: usize| match &tokens[sig[i]].token {
        Token::Word(w) => Some(w),
        _ => None,
    };
    let keyword =
        |i: usize, k: Keyword| word(i).is_some_and(|w| w.keyword == k && w.quote_style.is_none());
    if !keyword(0, Keyword::CREATE) {
        return None;
    }
    // CREATE [TEMP | UNLOGGED …] TABLE
    let mut i = (1..sig.len().min(4)).find(|&i| keyword(i, Keyword::TABLE))? + 1;
    if keyword(i, Keyword::IF) && keyword(i + 1, Keyword::NOT) && keyword(i + 2, Keyword::EXISTS) {
        i += 3;
    }
    // [schema .] name
    let mut name = word(i)?;
    while matches!(
        sig.get(i + 1).map(|&j| &tokens[j].token),
        Some(Token::Period)
    ) {
        i += 2;
        name = word(i)?;
    }
    let matches = if name.quote_style.is_some() {
        name.value == table_name
    } else {
        name.value.to_lowercase() == table_name
    };
    if !matches {
        return None;
    }
    i += 1;
    // [WITH ( … )]
    if keyword(i, Keyword::WITH)
        && matches!(
            sig.get(i + 1).map(|&j| &tokens[j].token),
            Some(Token::LParen)
        )
    {
        let mut depth = 0;
        i += 1;
        while i < sig.len() {
            match tokens[sig[i]].token {
                Token::LParen => depth += 1,
                Token::RParen => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        i += 1;
    }
    if !keyword(i, Keyword::AS) {
        return None;
    }
    let start = offsets[*sig.get(i + 1)?];
    let select = stmt_sql[start..].trim().trim_end_matches(';').trim();
    (!select.is_empty()).then(|| select.to_string())
}

#[cfg(test)]
mod ctas_extraction_tests {
    use super::extract_ctas_select;

    #[test]
    fn extracts_select_and_strips_semicolon() {
        let sql = "CREATE TABLE tv_post AS SELECT pk_post, id, data FROM tb_post;";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT pk_post, id, data FROM tb_post")
        );
    }

    #[test]
    fn handles_schema_if_not_exists_and_case() {
        let sql = "create table if not exists public.tv_post as\n  select 1";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("select 1")
        );
    }

    #[test]
    fn ignores_earlier_occurrences_of_the_name() {
        let sql = "/* tv_post as */ CREATE TABLE tv_post AS SELECT 'tv_post as x' FROM t";
        // A comment is read as one: the SELECT starts after the statement's AS.
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 'tv_post as x' FROM t")
        );
        let sql = "CREATE TABLE tv_post AS SELECT 'tv_post as x' FROM t";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 'tv_post as x' FROM t")
        );
    }

    #[test]
    fn does_not_match_a_different_table() {
        let sql = "CREATE TABLE tv_post_extra AS SELECT 1";
        assert_eq!(extract_ctas_select(sql, "tv_post"), None);
    }

    #[test]
    fn handles_quoted_names() {
        let sql = "CREATE TABLE \"s\".\"tv_post\" AS SELECT 1";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn skips_a_comment_with_an_apostrophe_after_as() {
        let sql = "CREATE TABLE tv_post AS -- don't\nSELECT 1;";
        assert_eq!(
            extract_ctas_select(sql, "tv_post").as_deref(),
            Some("SELECT 1")
        );
    }

    #[test]
    fn non_ascii_names_keep_offsets_on_char_boundaries() {
        let sql = "CREATE TABLE tv_café AS SELECT 'é' AS \"ü\"";
        assert_eq!(
            extract_ctas_select(sql, "tv_café").as_deref(),
            Some("SELECT 'é' AS \"ü\"")
        );
    }
}
