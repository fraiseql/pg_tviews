-- Real benchmark — base schema (command side).
--
-- Trinity naming the extension requires: id (UUID) + pk_<entity> + fk_<parent>.
-- No extensions, no v_/tv_/mv_ objects here — those are created per arm so the
-- same populated base data can seed every approach from a single template DB.

CREATE TABLE tb_category (
    id          uuid DEFAULT gen_random_uuid() NOT NULL,
    pk_category serial PRIMARY KEY,
    name        text NOT NULL,
    slug        text NOT NULL UNIQUE
);

CREATE TABLE tb_supplier (
    id          uuid DEFAULT gen_random_uuid() NOT NULL,
    pk_supplier serial PRIMARY KEY,
    name        text NOT NULL,
    country     text
);

CREATE TABLE tb_product (
    id            uuid DEFAULT gen_random_uuid() NOT NULL,
    pk_product    serial PRIMARY KEY,
    fk_category   integer NOT NULL REFERENCES tb_category(pk_category),
    fk_supplier   integer REFERENCES tb_supplier(pk_supplier),
    sku           text NOT NULL UNIQUE,
    name          text NOT NULL,
    description   text,
    base_price    numeric(10, 2) NOT NULL,
    current_price numeric(10, 2) NOT NULL,
    currency      text DEFAULT 'USD',
    status        text DEFAULT 'active'
);

CREATE TABLE tb_inventory (
    pk_inventory serial PRIMARY KEY,
    fk_product   integer NOT NULL UNIQUE REFERENCES tb_product(pk_product) ON DELETE CASCADE,
    quantity     integer DEFAULT 0,
    reserved     integer DEFAULT 0,
    warehouse_location text
);

CREATE TABLE tb_review (
    id                uuid DEFAULT gen_random_uuid() NOT NULL,
    pk_review         serial PRIMARY KEY,
    fk_product        integer NOT NULL REFERENCES tb_product(pk_product) ON DELETE CASCADE,
    fk_user           integer NOT NULL,
    rating            integer CHECK (rating BETWEEN 1 AND 5),
    title             text,
    content           text,
    verified_purchase boolean DEFAULT false,
    helpful_count     integer DEFAULT 0
);

CREATE INDEX idx_rb_product_category ON tb_product(fk_category);
CREATE INDEX idx_rb_product_supplier ON tb_product(fk_supplier);
CREATE INDEX idx_rb_review_product   ON tb_review(fk_product);
