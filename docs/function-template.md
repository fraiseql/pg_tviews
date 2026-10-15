# Function Documentation Template

Use this template for all function documentation in API reference pages.

---

## [Function Name]()

**Signature**:
```sql
function_name(parameter1 TYPE, parameter2 TYPE DEFAULT default) RETURNS RETURN_TYPE
```

**Description**:
One-sentence summary of what this function does. Focus on the "what" and "why", not implementation details.

**Parameters**:
- `parameter1` (TYPE): Description of first parameter, including valid values and constraints
- `parameter2` (TYPE, optional): Description with default value if applicable

**Returns**:
- `RETURN_TYPE`: Description of return value, including format and possible values

**Example**:
```sql
-- Clear, complete example with realistic data
SELECT function_name('value1', 'value2');
```

**Returns**:
```text
-- Expected output format
expected_result
```

**Notes**:
- Additional context or important details
- Performance characteristics if relevant
- Common pitfalls or limitations
- Version availability if not in all versions

**Errors**:
- **ErrorName**: When this error occurs and how to resolve it
- **AnotherError**: Another possible error scenario

**See Also** (links relative to the page, e.g. a page in `docs/reference/`):

```markdown
- [Related Function](related-function.md)
- [Usage Guide](../user-guides/developers.md)
- [Troubleshooting](../operations/troubleshooting.md#function-name)
```

---

## Template Usage Examples

### Simple Function

## pg_tviews_version()

**Signature**:
```sql
pg_tviews_version() RETURNS TEXT
```

**Description**:
Returns the version string of the pg_tviews extension.

**Parameters**:
- None

**Returns**:
- `TEXT`: Version string in format "major.minor.patch-suffix"

**Example**:
```sql
SELECT pg_tviews_version();
```

**Returns**:
```text
0.1.0-beta.1
```

### Complex Function

## pg_tviews_create()

**Signature**:
```sql
pg_tviews_create(tview TEXT, query TEXT, options JSONB DEFAULT '{}') RETURNS TEXT
```

**Description**:
Creates a new transactional view (TVIEW) from a SELECT statement with automatic incremental refresh capabilities.

**Parameters**:
- `tview` (TEXT): the TVIEW to create, as its entity (`post`), `tv_post` or `schema.tv_post`
- `query` (TEXT): Valid SELECT statement defining the TVIEW structure
- `options` (JSONB): the TVIEW's options (`logged`, `fillfactor`, `group_keys`, …); each one not passed is at its default

**Returns**:
- `TEXT`: Success message; errors are raised

**Example**:
```sql
SELECT pg_tviews_create('tv_post', '
    SELECT
        p.pk_post as pk_post,
        p.id,
        jsonb_build_object(
            ''title'', p.title,
            ''author'', jsonb_build_object(''id'', u.id, ''name'', u.name)
        ) as data
    FROM tb_post p
    JOIN tb_user u ON p.fk_user = u.pk_user
');
```

**Returns**:
```text
TVIEW public.tv_post created
```

**Notes**:
- The TVIEW's table is always `tv_<entity>`, `<entity>` taken from the `pk_<entity>` column
- SELECT statement must include required columns: `pk_<entity>`, `id`, `data`
- Triggers are automatically installed on source tables
- Performance: Initial creation time scales with data size

**Errors**:
- **22023**: the name does not match the definition's `pk_<entity>` column, or an option is invalid
- **0A000**: the SELECT contains features pg_tviews cannot maintain
- **42P07**: a TVIEW with this name already exists

**See Also**:
- [DROP TABLE tv_*](reference/ddl.md#drop-table-tv_)
- [TVIEW Creation Guide](getting-started/quickstart.md)
- [Troubleshooting](operations/troubleshooting.md#creation-fails)

---

## Guidelines for Template Use

### Signature
- Use exact PostgreSQL function signature
- Include parameter names, types, and defaults
- Match the actual function definition

### Description
- Start with action verb: "Returns", "Creates", "Checks"
- Be specific but concise
- Focus on user benefit, not implementation

### Parameters
- One bullet per parameter
- Include type and optionality
- Explain constraints and valid values
- Note defaults clearly

### Returns
- Describe the format/structure
- List possible values if not obvious
- Include examples for complex types

### Examples
- Use realistic data, not `foo`/`bar`
- Show complete, runnable code
- Include expected output
- Test examples before committing

### Notes
- Add context that doesn't fit elsewhere
- Include performance considerations
- Mention version requirements
- Warn about common mistakes

### Errors
- List the most common errors
- Explain when they occur
- Provide resolution steps
- Link to troubleshooting if detailed

### See Also
- Link to related functions
- Reference relevant guides
- Point to troubleshooting sections