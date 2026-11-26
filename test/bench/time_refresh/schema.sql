-- The time-refresh workload: seven TVIEWs that read the current time, at the row
-- counts of a pilot-sized deployment (about 28,100 rows), over a fleet domain:
-- organizations, their units and sites, agreements with their items, assets placed
-- at sites, add-ons on assets, network configurations of sites.
--
--   tv_agreement   34 rows, each embedding ~600 items (documents of ~40 kB)
--   tv_asset       5,880 rows: its agreement, its site, its current placement
--   tv_placement   6,960 rows: embeds tv_asset's document (a copy of a copy)
--   tv_addon       3,610 rows: its asset
--   tv_site        6,250 rows: active placements counted
--   tv_netconf     2,970 rows: its site
--   tv_unit        2,420 rows: its organization, active placements counted
--
-- Every one reads now(): an agreement or placement is current while now() lies in
-- its period. Periods end throughout the next minutes, so each boundary changes a
-- few hundred rows, as an hourly call changes the rows whose period ended.
--
-- Usage: psql -v ON_ERROR_STOP=1 -v scale=1 -f schema.sql   (scale multiplies rows)
\set ON_ERROR_STOP on
SET client_min_messages = warning;
\if :{?scale}
\else
\set scale 1
\endif
CREATE EXTENSION IF NOT EXISTS jsonb_delta;
CREATE EXTENSION IF NOT EXISTS pg_tviews;
SELECT setseed(0.41);

CREATE TABLE tb_org (pk_org bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                     name text NOT NULL);
CREATE TABLE tb_unit (pk_unit bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_org bigint NOT NULL REFERENCES tb_org, name text NOT NULL);
CREATE TABLE tb_site (pk_site bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                      fk_unit bigint NOT NULL REFERENCES tb_unit, name text NOT NULL,
                      address text NOT NULL);
CREATE TABLE tb_netconf (pk_netconf bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                         fk_site bigint NOT NULL REFERENCES tb_site, hostname text NOT NULL,
                         valid_until timestamptz NOT NULL);
CREATE TABLE tb_agreement (pk_agreement bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_org bigint NOT NULL REFERENCES tb_org, name text NOT NULL,
                           starts_at timestamptz NOT NULL, ends_at timestamptz NOT NULL);
CREATE TABLE tb_agreement_item (pk_item bigint PRIMARY KEY,
                                fk_agreement bigint NOT NULL REFERENCES tb_agreement,
                                label text NOT NULL, qty int NOT NULL, price numeric(10, 2) NOT NULL,
                                ends_at timestamptz NOT NULL);
CREATE TABLE tb_asset (pk_asset bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_agreement bigint NOT NULL REFERENCES tb_agreement,
                       fk_site bigint NOT NULL REFERENCES tb_site, serial text NOT NULL,
                       spec text NOT NULL, warranty_until timestamptz NOT NULL);
CREATE TABLE tb_placement (pk_placement bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                           fk_asset bigint NOT NULL REFERENCES tb_asset,
                           fk_site bigint NOT NULL REFERENCES tb_site, note text NOT NULL,
                           starts_at timestamptz NOT NULL, ends_at timestamptz NOT NULL);
CREATE TABLE tb_addon (pk_addon bigint PRIMARY KEY, id uuid NOT NULL DEFAULT gen_random_uuid(),
                       fk_asset bigint NOT NULL REFERENCES tb_asset, kind text NOT NULL,
                       until timestamptz NOT NULL);
CREATE INDEX ON tb_unit (fk_org);
CREATE INDEX ON tb_site (fk_unit);
CREATE INDEX ON tb_netconf (fk_site);
CREATE INDEX ON tb_agreement (fk_org);
CREATE INDEX ON tb_agreement_item (fk_agreement);
CREATE INDEX ON tb_asset (fk_agreement);
CREATE INDEX ON tb_asset (fk_site);
CREATE INDEX ON tb_placement (fk_asset);
CREATE INDEX ON tb_placement (fk_site);
CREATE INDEX ON tb_addon (fk_asset);

-- A period end: 5 % within the next 15 minutes (the rows a boundary changes), the
-- rest spread over 30 days on either side.
CREATE FUNCTION bench_end() RETURNS timestamptz LANGUAGE sql VOLATILE AS $$
    SELECT CASE WHEN random() < 0.05 THEN now() + random() * interval '15 minutes'
                ELSE now() + (random() * 60 - 30) * interval '1 day' END $$;

INSERT INTO tb_org (pk_org, name) SELECT g, 'org ' || g FROM generate_series(1, 18 * :scale) g;
INSERT INTO tb_unit (pk_unit, fk_org, name) SELECT g, 1 + (g % (18 * :scale)), 'unit ' || g
FROM generate_series(1, 2420 * :scale) g;
INSERT INTO tb_site (pk_site, fk_unit, name, address) SELECT g, 1 + (g % (2420 * :scale)), 'site ' || g,
                           repeat('street ' || g || ' ', 12)
FROM generate_series(1, 6250 * :scale) g;
INSERT INTO tb_netconf (pk_netconf, fk_site, hostname, valid_until) SELECT g, 1 + (g % (6250 * :scale)), 'host-' || g, bench_end()
FROM generate_series(1, 2970 * :scale) g;
INSERT INTO tb_agreement (pk_agreement, fk_org, name, starts_at, ends_at) SELECT g, 1 + (g % (18 * :scale)), 'agreement ' || g,
                                now() - interval '1 year', bench_end()
FROM generate_series(1, 34 * :scale) g;
INSERT INTO tb_agreement_item SELECT g, 1 + (g % (34 * :scale)), 'item ' || g, 1 + g % 7,
                                     (g % 997) / 3.0, bench_end()
FROM generate_series(1, 20400 * :scale) g;
INSERT INTO tb_asset (pk_asset, fk_agreement, fk_site, serial, spec, warranty_until) SELECT g, 1 + (g % (34 * :scale)), 1 + (g % (6250 * :scale)),
                            'SN' || lpad(g::text, 8, '0'), repeat('spec ' || g || ' ', 40),
                            bench_end()
FROM generate_series(1, 5880 * :scale) g;
INSERT INTO tb_placement (pk_placement, fk_asset, fk_site, note, starts_at, ends_at) SELECT g, 1 + (g % (5880 * :scale)), 1 + ((g * 7) % (6250 * :scale)),
                                repeat('note ' || g || ' ', 30), now() - interval '90 days', bench_end()
FROM generate_series(1, 6960 * :scale) g;
INSERT INTO tb_addon (pk_addon, fk_asset, kind, until) SELECT g, 1 + (g % (5880 * :scale)), 'kind ' || (g % 11), bench_end()
FROM generate_series(1, 3610 * :scale) g;
ANALYZE;

SELECT tviews.pg_tviews_create_or_replace('public.tv_agreement', $$
    SELECT a.pk_agreement, a.id,
           jsonb_build_object('name', a.name, 'org', o.name,
                              'is_current', a.starts_at <= now() AND now() < a.ends_at,
                              'items', (SELECT jsonb_agg(jsonb_build_object(
                                                  'label', i.label, 'qty', i.qty, 'price', i.price,
                                                  'is_active', now() < i.ends_at)
                                                ORDER BY i.pk_item)
                                        FROM tb_agreement_item i
                                        WHERE i.fk_agreement = a.pk_agreement)) AS data
    FROM tb_agreement a JOIN tb_org o ON o.pk_org = a.fk_org $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_asset', $$
    SELECT s.pk_asset, s.id,
           jsonb_build_object('serial', s.serial, 'spec', s.spec,
                              'warranty_active', now() < s.warranty_until,
                              'site', si.name,
                              'agreement', jsonb_build_object(
                                  'name', a.name,
                                  'is_current', a.starts_at <= now() AND now() < a.ends_at),
                              'placement', (SELECT jsonb_build_object('note', p.note,
                                                                      'ends_at', p.ends_at)
                                            FROM tb_placement p
                                            WHERE p.fk_asset = s.pk_asset
                                              AND p.starts_at <= now() AND now() < p.ends_at
                                            ORDER BY p.pk_placement LIMIT 1)) AS data
    FROM tb_asset s JOIN tb_agreement a ON a.pk_agreement = s.fk_agreement
                    JOIN tb_site si ON si.pk_site = s.fk_site $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_placement', $$
    SELECT p.pk_placement, p.id,
           jsonb_build_object('note', p.note,
                              'is_active', p.starts_at <= now() AND now() < p.ends_at,
                              'site', si.name, 'address', si.address, 'unit', u.name,
                              'asset', ta.data) AS data
    FROM tb_placement p JOIN tv_asset ta ON ta.pk_asset = p.fk_asset
                        JOIN tb_site si ON si.pk_site = p.fk_site
                        JOIN tb_unit u ON u.pk_unit = si.fk_unit $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_addon', $$
    SELECT d.pk_addon, d.id,
           jsonb_build_object('kind', d.kind, 'is_active', now() < d.until,
                              'serial', s.serial,
                              'warranty_active', now() < s.warranty_until) AS data
    FROM tb_addon d JOIN tb_asset s ON s.pk_asset = d.fk_asset $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_site', $$
    SELECT si.pk_site, si.id,
           jsonb_build_object('name', si.name, 'address', si.address, 'unit', u.name,
                              'active_placements',
                              (SELECT count(*) FROM tb_placement p
                               WHERE p.fk_site = si.pk_site
                                 AND p.starts_at <= now() AND now() < p.ends_at)) AS data
    FROM tb_site si JOIN tb_unit u ON u.pk_unit = si.fk_unit $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_netconf', $$
    SELECT n.pk_netconf, n.id,
           jsonb_build_object('hostname', n.hostname, 'valid', now() < n.valid_until,
                              'site', si.name, 'address', si.address) AS data
    FROM tb_netconf n JOIN tb_site si ON si.pk_site = n.fk_site $$,
    '{"time_refresh": "external"}');

SELECT tviews.pg_tviews_create_or_replace('public.tv_unit', $$
    SELECT u.pk_unit, u.id,
           jsonb_build_object('name', u.name, 'org', o.name,
                              'active_placements',
                              (SELECT count(*) FROM tb_placement p JOIN tb_site si
                                                  ON si.pk_site = p.fk_site
                               WHERE si.fk_unit = u.pk_unit
                                 AND p.starts_at <= now() AND now() < p.ends_at)) AS data
    FROM tb_unit u JOIN tb_org o ON o.pk_org = u.fk_org $$,
    '{"time_refresh": "external"}');
ANALYZE;

-- The saves of the 40/20/20/20 mix, each as a mutation function: one write, then
-- the document read back.
CREATE FUNCTION save_placement(k bigint) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_placement SET note = 'note ' || k || ' ' || clock_timestamp() WHERE pk_placement = k;
    RETURN (SELECT data FROM tv_placement WHERE pk_placement = k);
END $$;
CREATE FUNCTION save_asset(k bigint) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_asset SET serial = 'SN' || lpad(k::text, 8, '0') || '-' || (random() * 9)::int
    WHERE pk_asset = k;
    RETURN (SELECT data FROM tv_asset WHERE pk_asset = k);
END $$;
CREATE FUNCTION save_site(k bigint) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_site SET name = 'site ' || k || '-' || (random() * 9)::int WHERE pk_site = k;
    RETURN (SELECT data FROM tv_site WHERE pk_site = k);
END $$;
CREATE FUNCTION save_addon(k bigint) RETURNS jsonb LANGUAGE plpgsql AS $$
BEGIN
    UPDATE tb_addon SET kind = 'kind ' || (random() * 11)::int WHERE pk_addon = k;
    RETURN (SELECT data FROM tv_addon WHERE pk_addon = k);
END $$;

-- Lock waits sampled during a run (run.sh), every 100 ms.
CREATE UNLOGGED TABLE bench_waits (at timestamptz NOT NULL, locktype text, mode text,
                                   relation text, field4 int, waiting int NOT NULL);
CREATE PROCEDURE bench_sample(seconds int) LANGUAGE plpgsql AS $$
DECLARE stop timestamptz := clock_timestamp() + seconds * interval '1 second';
BEGIN
    WHILE clock_timestamp() < stop LOOP
        INSERT INTO bench_waits
        SELECT clock_timestamp(), l.locktype, l.mode, l.relation::regclass::text,
               CASE l.locktype WHEN 'advisory' THEN l.objsubid END, count(*)
        FROM pg_locks l WHERE NOT l.granted
        GROUP BY 2, 3, 4, 5;
        COMMIT;
        PERFORM pg_sleep(0.1);
    END LOOP;
END $$;
