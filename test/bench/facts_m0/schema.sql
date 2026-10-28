-- ADR 0260 M0: one fact family kept at several grains by generated SQL, on a
-- database without pg_tviews_facts. The day grain is partitioned by year, as
-- statistics schemas partition it.
CREATE SCHEMA facts_m0;

CREATE SEQUENCE tf_sales_day_pk;
CREATE TABLE tf_sales_day (
    pk bigint NOT NULL DEFAULT nextval('tf_sales_day_pk'),
    tenant_id uuid NOT NULL,
    entity_id bigint NOT NULL,
    category text,
    day date NOT NULL,
    amount numeric(14, 2),
    qty bigint,
    PRIMARY KEY (pk, day)
) PARTITION BY RANGE (day);
CREATE TABLE tf_sales_day_2025 PARTITION OF tf_sales_day FOR VALUES FROM ('2025-01-01') TO ('2026-01-01');
CREATE TABLE tf_sales_day_2026 PARTITION OF tf_sales_day FOR VALUES FROM ('2026-01-01') TO ('2027-01-01');
CREATE TABLE tf_sales_day_2027 PARTITION OF tf_sales_day FOR VALUES FROM ('2027-01-01') TO ('2028-01-01');

-- The family as FraiseQL's spec would declare it (ADR 0260 "Declaration"): the
-- day grain, two coarser grains keeping every dimension, and a rollup that
-- drops the entity but never the tenant. Additive measures only (M0).
CREATE TABLE facts_m0.family (name text PRIMARY KEY, declaration jsonb NOT NULL);
INSERT INTO facts_m0.family VALUES ('sales', $${
  "family": "sales",
  "source": "public.tf_sales_day",
  "time_column": "day",
  "tenant": "tenant_id",
  "measures": ["amount", "qty"],
  "lock": true,
  "grains": [
    {"table": "public.tf_sales_month", "grain": "month", "dimensions": ["entity_id", "category"]},
    {"table": "public.tf_sales_year", "grain": "year", "dimensions": ["entity_id", "category"]},
    {"table": "public.tf_sales_category_month", "grain": "month", "dimensions": ["category"]}
  ]
}$$);
