-- ADR 0260 M0: verify(family) recomputes every derived table from the source and
-- returns each row that differs, both ways: a row the table lacks or holds wrong
-- (`expected`), and a row it holds that the source does not give (`unexpected`).
-- No row: the family is consistent.
CREATE FUNCTION facts_m0.verify(name text)
RETURNS TABLE (grain_table text, problem text, row_data jsonb)
LANGUAGE plpgsql STABLE AS $$
DECLARE
  f jsonb := (SELECT declaration FROM facts_m0.family WHERE family.name = verify.name);
  g jsonb;
  expected text;
BEGIN
  FOR g IN SELECT * FROM jsonb_array_elements(f->'grains') LOOP
    expected := facts_m0.expected_sql(f, g);
    RETURN QUERY EXECUTE format(
      'SELECT %1$L, ''expected'', to_jsonb(x) FROM (%2$s EXCEPT SELECT * FROM %1$s) x '
      'UNION ALL '
      'SELECT %1$L, ''unexpected'', to_jsonb(y) FROM (SELECT * FROM %1$s EXCEPT %2$s) y',
      g->>'table', expected);
  END LOOP;
END $$;

-- fraiseql #1798: a bucket of a `date` is a `date`, whatever the session's
-- TimeZone. Inserts one row dated 2026-07-01 under two zones far from UTC (one on
-- each side) and returns each bucket that does not read as its date, then takes
-- the row out again. No row: the buckets never move with the zone.
CREATE FUNCTION facts_m0.check_zones(name text)
RETURNS TABLE (zone text, grain_table text, bucket text)
LANGUAGE plpgsql AS $$
DECLARE
  f jsonb := (SELECT declaration FROM facts_m0.family WHERE family.name = check_zones.name);
  g jsonb;
  z text;
  want text;
  tenant uuid := md5('check_zones')::uuid;
BEGIN
  FOREACH z IN ARRAY ARRAY['Pacific/Kiritimati', 'America/Adak'] LOOP
    PERFORM set_config('TimeZone', z, true);
    EXECUTE format('INSERT INTO %s (%I, entity_id, category, %I, amount, qty) VALUES ($1, 0, NULL, ''2026-07-01'', 1, 1)',
                   f->>'source', f->>'tenant', f->>'time_column') USING tenant;
    FOR g IN SELECT * FROM jsonb_array_elements(f->'grains') LOOP
      want := CASE g->>'grain' WHEN 'year' THEN '2026-01-01' WHEN 'semester' THEN '2026-07-01'
                               WHEN 'quarter' THEN '2026-07-01' WHEN 'iso_week' THEN '2026-06-29'
                               ELSE '2026-07-01' END;
      RETURN QUERY EXECUTE format('SELECT %L::text, %L::text, bucket::text FROM %s WHERE %I = $1 AND bucket::text <> $2',
                                  z, g->>'table', g->>'table', f->>'tenant') USING tenant, want;
    END LOOP;
    EXECUTE format('DELETE FROM %s WHERE %I = $1', f->>'source', f->>'tenant') USING tenant;
  END LOOP;
END $$;
