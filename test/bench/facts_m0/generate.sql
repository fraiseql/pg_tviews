-- ADR 0260 M0: the SQL the companion would generate for a declared family.
-- facts_m0.generate(declaration) returns the statements: derived tables, the
-- statement trigger functions (one per operation) and their triggers on the
-- source and every partition. facts_m0.install(name) runs them and fills the
-- derived tables from the rows already there.

-- The bucket a day falls in, at `grain` (fraiseql #1793's period starts).
CREATE FUNCTION facts_m0.bucket_sql(grain text, col text) RETURNS text
LANGUAGE sql IMMUTABLE AS $$
  -- Truncated as a timestamp without time zone and cast back: a bucket of a date
  -- is a date, never local midnight of the session's TimeZone (fraiseql #1798).
  SELECT CASE grain
    WHEN 'day' THEN format('%I', col)
    WHEN 'half_month' THEN format(
      'CASE WHEN EXTRACT(DAY FROM %1$I) <= 15 THEN date_trunc(''month'', %1$I::timestamp)::date '
      'ELSE date_trunc(''month'', %1$I::timestamp)::date + 15 END', col)
    WHEN 'month' THEN format('date_trunc(''month'', %I::timestamp)::date', col)
    WHEN 'quarter' THEN format('date_trunc(''quarter'', %I::timestamp)::date', col)
    WHEN 'semester' THEN format(
      'make_date(EXTRACT(YEAR FROM %1$I)::int, CASE WHEN EXTRACT(MONTH FROM %1$I) <= 6 THEN 1 ELSE 7 END, 1)', col)
    WHEN 'year' THEN format('date_trunc(''year'', %I::timestamp)::date', col)
    WHEN 'iso_week' THEN format('date_trunc(''week'', %I::timestamp)::date', col)
  END $$;

-- A quoted, comma-separated column list, each prefixed with `prefix`.
CREATE FUNCTION facts_m0.cols(names text[], prefix text DEFAULT '') RETURNS text
LANGUAGE sql IMMUTABLE AS $$
  SELECT string_agg(prefix || quote_ident(n), ', ' ORDER BY o)
  FROM unnest(names) WITH ORDINALITY u(n, o) $$;

-- The SQL type of `col` in relation `rel`.
CREATE FUNCTION facts_m0.col_type(rel regclass, col text) RETURNS text
LANGUAGE sql STABLE AS $$
  SELECT format_type(atttypid, atttypmod) FROM pg_attribute
  WHERE attrelid = rel AND attname = col AND NOT attisdropped $$;

CREATE FUNCTION facts_m0.generate(f jsonb) RETURNS text[]
LANGUAGE plpgsql AS $gen$
DECLARE
  fam text := f->>'family';
  src regclass := (f->>'source')::regclass;
  tcol text := f->>'time_column';
  tenant text := f->>'tenant';
  measures text[] := ARRAY(SELECT jsonb_array_elements_text(f->'measures'));
  lock boolean := COALESCE((f->>'lock')::boolean, true);
  g jsonb;
  tbl text;
  dims text[];
  keys text[];
  bucket text;
  btype text;
  m text;
  out text[] := '{}';
  body jsonb := '{"INSERT": "", "UPDATE": "", "DELETE": ""}';
  op text;
  delta text;
  sums text;
  nonzero text;
  sets text;
  ddl text;
  rel regclass;
BEGIN
  FOR g IN SELECT * FROM jsonb_array_elements(f->'grains') LOOP
    tbl := g->>'table';
    dims := ARRAY(SELECT jsonb_array_elements_text(g->'dimensions'));
    keys := ARRAY[tenant] || dims;
    bucket := facts_m0.bucket_sql(g->>'grain', tcol);
    EXECUTE format('SELECT pg_typeof(%s)::text FROM (SELECT NULL::date AS %I) s', bucket, tcol)
      INTO btype;

    -- The derived table: its keys, the bucket, per measure the sum (NULLs as 0)
    -- and the count of non-NULL values, and the fact-row count _n.
    ddl := format('CREATE TABLE %s (', tbl);
    FOREACH m IN ARRAY keys LOOP
      ddl := ddl || format('%I %s, ', m, facts_m0.col_type(src, m));
    END LOOP;
    ddl := ddl || format('bucket %s NOT NULL, ', btype);
    FOREACH m IN ARRAY measures LOOP
      ddl := ddl || format('%I numeric NOT NULL DEFAULT 0, %I bigint NOT NULL DEFAULT 0, ', m, m || '_n');
    END LOOP;
    ddl := ddl || format('_n bigint NOT NULL, UNIQUE NULLS NOT DISTINCT (%s, bucket))',
                         facts_m0.cols(keys));
    out := out || ddl;
    -- Emptied buckets are found by tenant through a tiny partial index.
    out := out || format('CREATE INDEX ON %s (%I) WHERE _n = 0', tbl, tenant);

    -- One upsert of signed sums per derived table, then the emptied buckets go.
    sums := '';
    nonzero := 'sum(sgn) <> 0';
    sets := '_n = t._n + excluded._n';
    FOREACH m IN ARRAY measures LOOP
      sums := sums || format(', sum(sgn * COALESCE(%1$I, 0)), sum(sgn * (%1$I IS NOT NULL)::int)', m);
      nonzero := nonzero || format(' OR sum(sgn * COALESCE(%1$I, 0)) <> 0 OR sum(sgn * (%1$I IS NOT NULL)::int) <> 0', m);
      sets := sets || format(', %1$I = t.%1$I + excluded.%1$I, %2$I = t.%2$I + excluded.%2$I', m, m || '_n');
    END LOOP;
    FOREACH op IN ARRAY ARRAY['INSERT', 'UPDATE', 'DELETE'] LOOP
      delta := CASE op
        WHEN 'INSERT' THEN format('SELECT %s, %s AS bucket, %s, 1 AS sgn FROM n',
                                  facts_m0.cols(keys), bucket, facts_m0.cols(measures))
        WHEN 'DELETE' THEN format('SELECT %s, %s AS bucket, %s, -1 AS sgn FROM o',
                                  facts_m0.cols(keys), bucket, facts_m0.cols(measures))
        ELSE format('SELECT %1$s, %2$s AS bucket, %3$s, 1 AS sgn FROM n UNION ALL '
                    'SELECT %1$s, %2$s AS bucket, %3$s, -1 AS sgn FROM o',
                    facts_m0.cols(keys), bucket, facts_m0.cols(measures))
      END;
      body := jsonb_set(body, ARRAY[op], to_jsonb(body->>op || format(
        E'  INSERT INTO %1$s AS t (%2$s, bucket, %3$s, _n)\n'
        '  SELECT %2$s, bucket%4$s, sum(sgn) FROM (%5$s) d\n'
        '  GROUP BY %2$s, bucket HAVING %6$s ORDER BY %2$s, bucket\n'
        '  ON CONFLICT (%2$s, bucket) DO UPDATE SET %7$s;\n'
        '  DELETE FROM %1$s WHERE _n = 0 AND %8$I IN (SELECT %8$I FROM %9$s);\n',
        tbl, facts_m0.cols(keys),
        (SELECT string_agg(format('%I, %I', x, x || '_n'), ', ') FROM unnest(measures) x),
        sums, delta, nonzero, sets, tenant,
        CASE op WHEN 'INSERT' THEN 'n' WHEN 'DELETE' THEN 'o'
                ELSE format('(SELECT %1$I FROM n UNION SELECT %1$I FROM o) s', tenant) END)));
    END LOOP;
  END LOOP;

  -- One function per operation: lock the family's tenants in order, then apply.
  FOREACH op IN ARRAY ARRAY['INSERT', 'UPDATE', 'DELETE'] LOOP
    out := out || format(
      E'CREATE FUNCTION facts_m0.%1$I() RETURNS trigger LANGUAGE plpgsql AS $f$\nBEGIN\n%2$s%3$s  RETURN NULL;\nEND $f$',
      fam || '_' || lower(op),
      CASE WHEN lock THEN format(
        E'  PERFORM pg_advisory_xact_lock(hashtext(%1$L), hashtext(t::text))\n'
        '  FROM (SELECT DISTINCT %2$I AS t FROM %3$s ORDER BY 1) s;\n',
        fam, tenant,
        CASE op WHEN 'INSERT' THEN 'n' WHEN 'DELETE' THEN 'o'
                ELSE format('(SELECT %1$I FROM n UNION ALL SELECT %1$I FROM o) u', tenant) END)
      ELSE '' END,
      body->>op);
  END LOOP;

  -- Statement triggers on the source and on every partition: a statement fires
  -- only the triggers of the table it names.
  FOR rel IN SELECT relid FROM pg_partition_tree(src) LOOP
    out := out || format('CREATE TRIGGER %I AFTER INSERT ON %s REFERENCING NEW TABLE AS n '
                         'FOR EACH STATEMENT EXECUTE FUNCTION facts_m0.%I()',
                         'facts_m0_' || fam || '_i', rel, fam || '_insert');
    out := out || format('CREATE TRIGGER %I AFTER UPDATE ON %s REFERENCING OLD TABLE AS o NEW TABLE AS n '
                         'FOR EACH STATEMENT EXECUTE FUNCTION facts_m0.%I()',
                         'facts_m0_' || fam || '_u', rel, fam || '_update');
    out := out || format('CREATE TRIGGER %I AFTER DELETE ON %s REFERENCING OLD TABLE AS o '
                         'FOR EACH STATEMENT EXECUTE FUNCTION facts_m0.%I()',
                         'facts_m0_' || fam || '_d', rel, fam || '_delete');
  END LOOP;
  RETURN out;
END $gen$;

-- Create the family's derived tables and triggers, and fill the tables from the
-- source's current rows (the source is locked against writes meanwhile).
CREATE FUNCTION facts_m0.install(name text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
  f jsonb := (SELECT declaration FROM facts_m0.family WHERE family.name = install.name);
  s text;
BEGIN
  EXECUTE format('LOCK TABLE %s IN SHARE MODE', f->>'source');
  FOREACH s IN ARRAY facts_m0.generate(f) LOOP
    EXECUTE s;
  END LOOP;
  PERFORM facts_m0.refill(name);
END $$;

-- Recompute every derived table of the family from the source (rebuild).
CREATE FUNCTION facts_m0.refill(name text) RETURNS void
LANGUAGE plpgsql AS $$
DECLARE
  f jsonb := (SELECT declaration FROM facts_m0.family WHERE family.name = refill.name);
  g jsonb;
BEGIN
  FOR g IN SELECT * FROM jsonb_array_elements(f->'grains') LOOP
    EXECUTE format('DELETE FROM %s', g->>'table');
    EXECUTE format('INSERT INTO %s %s', g->>'table', facts_m0.expected_sql(f, g));
  END LOOP;
END $$;

-- The rows grain `g` of family `f` must hold, recomputed from the source.
CREATE FUNCTION facts_m0.expected_sql(f jsonb, g jsonb) RETURNS text
LANGUAGE plpgsql STABLE AS $$
DECLARE
  keys text[] := ARRAY[f->>'tenant'] || ARRAY(SELECT jsonb_array_elements_text(g->'dimensions'));
  m text;
  sums text := '';
BEGIN
  FOREACH m IN ARRAY ARRAY(SELECT jsonb_array_elements_text(f->'measures')) LOOP
    sums := sums || format(', COALESCE(sum(%1$I), 0) AS %1$I, count(%1$I) AS %2$I', m, m || '_n');
  END LOOP;
  RETURN format('SELECT %1$s, %2$s AS bucket%3$s, count(*) AS _n FROM %4$s GROUP BY %1$s, bucket',
                facts_m0.cols(keys), facts_m0.bucket_sql(g->>'grain', f->>'time_column'),
                sums, f->>'source');
END $$;
