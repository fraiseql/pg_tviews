# Documentation Style Guide

**Version**: 1.0 • **Last Updated**: December 11, 2025
**Applies to**: All pg_tviews documentation

---

## Purpose

This style guide ensures consistent, professional documentation that is easy to read, maintain, and contribute to. All documentation should follow these standards for a cohesive user experience.

## General Principles

### Clarity First
- **Write for beginners**: Assume no prior knowledge of pg_tviews
- **Be concise**: Remove unnecessary words without losing meaning
- **Use active voice**: "The function creates a TVIEW" not "A TVIEW is created by the function"
- **Avoid jargon**: Explain technical terms or link to explanations

### Structure for Scannability
- **Use headings**: Break content into logical sections
- **Short paragraphs**: Maximum 4-5 sentences per paragraph
- **Bullet points**: For lists, steps, and features
- **Code examples**: Show, don't tell

### Consistency
- **Terminology**: Use consistent terms throughout (see Terminology section)
- **Formatting**: Follow the same patterns for similar content
- **Voice**: Maintain professional, helpful tone

---

## Formatting Standards

### Headings

```markdown
# Page Title (H1 - only one per page)

## Major Section (H2)

### Subsection (H3)

#### Sub-subsection (H4 - rarely needed)
```

**Rules**:
- Use sentence case for headings
- No skipped levels (H1 → H3 is invalid)
- Maximum 4 heading levels per document

### Code Blocks

**SQL Code**:
```sql
-- Use sql language identifier
SELECT pg_tviews_create('tv_example', 'SELECT 1 as data');
```

**Bash Commands**:
```bash
# Use bash language identifier
cargo pgrx install --release
```

**Output**:
```text
-- Use text for command output
TVIEW 'tv_example' created successfully
```

**Rules**:
- Always specify language for syntax highlighting
- Include expected output for examples
- Test all code examples before committing
- Use realistic data, not `foo`/`bar`

### Inline Code

- **Function names**: `pg_tviews_create()`
- **Parameters**: `tview_name`, `select_sql`
- **File paths**: `docs/reference/api.md`
- **Commands**: `cargo pgrx install`

### Links

**Internal links**:
```markdown
[API Reference](api.md)
[Installation Guide](../getting-started/installation.md)
```

**External links**:
```markdown
[PostgreSQL Documentation](https://www.postgresql.org/docs/)
```

**Rules**:
- Use relative paths for internal links
- Test all links before committing
- Use descriptive link text

### Tables

```markdown
| Column 1 | Column 2 | Description |
|----------|----------|-------------|
| Value A  | Value B  | Description of A and B |
| Value C  | Value D  | Description of C and D |
```

**Rules**:
- Left-align text columns
- Right-align numeric columns
- Include header row
- Keep tables narrow (3-4 columns max)

### Lists

**Bulleted lists**:
- Use for unordered information
- Start with capital letters
- End with periods for complete sentences

**Numbered lists**:
1. Use for sequential steps
2. Use for ordered information
3. Number automatically in Markdown

---

## Terminology

### Official Terms

| Term | Usage | Example |
|------|-------|---------|
| **TVIEW** | The feature/technology | "TVIEWs provide incremental refresh" |
| **tv_post** | Specific TVIEW name | "Create TABLE `tv_post`" |
| **pg_tviews** | The extension name | "Install pg_tviews extension" |
| **FraiseQL** | The framework | "Part of the FraiseQL framework" |
| **trinity pattern** | Identifier pattern | "Follow the trinity pattern" |

### Avoid These Terms

| Instead of | Use |
|------------|-----|
| "Simply" | Remove or rephrase |
| "Just" | Remove or rephrase |
| "Obviously" | Remove |
| "Easy" | Remove |
| "Basically" | Remove |
| "TODO" | Fix or remove |
| "FIXME" | Fix or remove |

### PostgreSQL Terms

| Term | Meaning |
|------|---------|
| **Materialized view** | Traditional PostgreSQL MV |
| **Transactional materialized view** | What TVIEWs provide |
| **Incremental refresh** | Row-level updates |
| **Trigger** | Database trigger |
| **Extension** | PostgreSQL extension |

---

## Document Structure

### Standard Document Header

```markdown
# Document Title

Brief one-paragraph description of what this document covers.

**Version**: 0.1.0-beta.1 • **Last Updated**: YYYY-MM-DD

## Table of Contents

- [Section 1](#section-1)
- [Section 2](#section-2)

## Section 1
...
```

### Page Types

**Reference Pages** (API, DDL, etc.):
1. Overview paragraph
2. Table of contents
3. Main content sections
4. See Also section

**Guide Pages** (Installation, tutorials):
1. Overview paragraph
2. Prerequisites
3. Step-by-step instructions
4. Troubleshooting
5. Next steps

**Conceptual Pages** (Architecture, performance):
1. Overview paragraph
2. Key concepts
3. Detailed explanations
4. Examples
5. Related topics

---

## Code Examples

### SQL Example Standards

**All SQL examples MUST follow the trinity pattern**:

- ✅ **Singular table names**: `tb_post`, `tv_post`, `v_post` (NOT `tb_posts`)
- ✅ **Qualified columns**: `tb_post.id` (NOT just `id`)
- ✅ **pk_*/fk_* are INTEGER**: Internal database operations
- ✅ **id is UUID**: External API/GraphQL identifiers
- ✅ **JSONB uses camelCase**: `'userId'` (NOT `'user_id'`)

**Correct example**:
```sql
SELECT
    tb_post.pk_post,          -- INTEGER primary key
    tb_post.id,               -- UUID for GraphQL
    tb_post.fk_user,          -- INTEGER foreign key
    jsonb_build_object(
        'id', tb_post.id,     -- camelCase keys
        'title', tb_post.title,
        'userId', tb_user.id  -- Related UUID from JOIN
    ) as data
FROM tb_post
INNER JOIN tb_user ON tb_post.fk_user = tb_user.pk_user;
```

**Incorrect examples** (DO NOT USE):
```sql
-- ❌ Unqualified columns
SELECT id as pk_post, jsonb_build_object('id', id) as data FROM tb_post;

-- ❌ Plural table names
SELECT tb_post.pk_post, jsonb_build_object('id', tb_post.id) as data FROM tb_post;

-- ❌ snake_case in JSONB
SELECT tb_post.pk_post, jsonb_build_object('user_id', tb_user.id) as data FROM tb_post;
```

### Good Examples

**Complete and tested**:
```sql
-- Create test table
CREATE TABLE tb_user (
    pk_user BIGSERIAL PRIMARY KEY,
    id UUID DEFAULT gen_random_uuid(),
    name TEXT NOT NULL
);

-- Create TVIEW
SELECT pg_tviews_create('tv_user', '
SELECT pk_user, id, jsonb_build_object(''name'', name) as data
FROM tb_user
');

-- Verify it works
INSERT INTO tb_user (name) VALUES ('Alice');
SELECT * FROM tv_user;
```

**Shows expected output**:
```text
 pk_user |                  id                  |              data
---------+--------------------------------------+--------------------------------
       1 | 123e4567-e89b-12d3-a456-426614174000 | {"name": "Alice"}
```

### Bad Examples

❌ **No output shown**:
```sql
SELECT pg_tviews_create('tv_user', '...');
-- What should this return?
```

❌ **Untested placeholders**:
```sql
SELECT pg_tviews_create('tv_example', 'SELECT * FROM some_table');
-- ERROR: table some_table does not exist
```

❌ **Generic data**:
```sql
INSERT INTO users (name) VALUES ('foo');
-- Use realistic data like 'Alice' or 'Bob'
```

---

## Error Messages and Warnings

### User-Facing Errors

**Show actual error messages**:
```sql
-- This will fail:
CREATE TABLE tv_invalid (id INT);
-- ERROR: TVIEW name must start with tv_
```

**Explain the error**:
> **Error**: TVIEW names must follow the `tv_*` naming convention.
> **Solution**: Rename to `tv_invalid` or choose a different name.

### Internal Errors

**For debugging docs only**:
```sql
-- Internal error (users should not see this):
-- ERROR: MetadataNotFound: TVIEW metadata not found for entity 'posts'
```

---

## File Organization

### Directory Structure

```
docs/
├── getting-started/     # Installation, quickstart
├── user-guides/        # How-to guides for personas
├── reference/          # API, DDL, error references
├── operations/         # Monitoring, troubleshooting
├── style-guide.md      # This file
└── README.md           # Documentation index
```

### File Naming

- Use kebab-case: `api-reference.md`, `installation-guide.md`
- Be descriptive: `troubleshooting.md` not `problems.md`
- Use consistent prefixes: `api.md`, `ddl.md`, `errors.md`

---

## Review Checklist

Before committing documentation changes:

### Content
- [ ] Technically accurate (verified against code)
- [ ] All examples tested and working
- [ ] All links valid
- [ ] No typos (spell check run)
- [ ] Appropriate level of detail

### Structure
- [ ] Header with version/date
- [ ] Table of contents (if >500 lines)
- [ ] Proper heading hierarchy
- [ ] Code blocks properly formatted
- [ ] Consistent terminology

### Quality
- [ ] Clear and concise
- [ ] Appropriate examples
- [ ] Cross-references added
- [ ] Follows style guide
- [ ] Accessible (alt text for images)

---

## Tools and Automation

### Linting

Run markdownlint on all documentation:

```bash
# Install markdownlint
npm install -g markdownlint-cli

# Check all docs
markdownlint docs/**/*.md
```

### Link Checking

```bash
# Install link checker
npm install -g markdown-link-check

# Check links
find docs -name "*.md" -exec markdown-link-check {} \;
```

### Example Testing

All code examples should be tested in CI. See `.phases/documentation/APLUS_DOCUMENTATION_PLAN_PART3.md` for automated testing setup.

---

## Contributing

### For New Documents

1. Follow this style guide
2. Use appropriate template (see below)
3. Add to table of contents
4. Test all examples
5. Get review from maintainer

### For Updates

1. Check style guide compliance
2. Update version/date in header
3. Test any new examples
4. Ensure cross-references still valid

---

## Templates

See associated template files:
- [Function Documentation Template](function-template.md)
- [Document Header Template](document-template.md)
- [Review Checklist Template](review-checklist.md)

---

**Remember**: Good documentation is hard work, but it pays dividends in user satisfaction and reduced support burden.