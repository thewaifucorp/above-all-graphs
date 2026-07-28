---
wiki: src/database.rs
---

# database.rs

Live `PostgreSQL` catalog ingestion. This is P1.8 of
[capability coverage](capability-coverage.md).

A repository's `.sql` files say what the schema was meant to be. The server
says what it is. Both belong in the graph, and they belong there separately:
DDL is indexed as `Perspective::Declared` by [artifacts](storage.md), a live
catalog as `Perspective::Observed` by this module, and `aag db drift` reports
the two ways they disagree rather than quietly preferring one.

## Reading a catalog

```bash
aag db scan --url "postgres://app@db.internal/shop"
aag db scan                       # AAG_DATABASE_URL, then DATABASE_URL
aag db drift                      # declared DDL vs the ingested catalog
```

Four `pg_catalog` queries, no ORM and no `information_schema` round-trip:

| Query | What it reads |
|---|---|
| `pg_class` + `pg_namespace` | tables, partitioned tables, foreign tables, views, materialized views, and `COMMENT ON TABLE` |
| `pg_attribute` + `pg_attrdef` | columns in ordinal order, `format_type` as the server prints it, nullability, default expression |
| `pg_constraint` | primary keys, unique constraints, `CHECK` definitions, and foreign keys with both column lists in order |
| `pg_index` | every index, its columns in order, whether it is unique, whether it backs the primary key |

Composite keys keep their column order because `conkey`/`confkey` are unnested
`WITH ORDINALITY` — a foreign key over `(tenant, id)` matching `(id, tenant)`
would be a wrong answer that looks right.

The server's own schemas (`pg_catalog`, `information_schema`, `pg_toast`) are
excluded: they are the server's, not the application's.

## What lands in the graph

| Catalog object | Node | Edge |
|---|---|---|
| Table, view, materialized view | `DatabaseTable`, named `schema.table` | — |
| Column | `DatabaseColumn`, named `schema.table.column` | table `References` column |
| Foreign key | — | table `References` table, and column `References` column |

The same `DatabaseTable` kind and `References` edge DDL ingestion already
produces, so a query does not need to know which half it is reading:

```cypher
MATCH (t:DatabaseTable)-[:REFERENCES]->(c:DatabaseColumn) RETURN t.name, collect(c.name)
```

Details that are properties rather than symbols ride on the node's description
as JSON: a table carries its relation kind, comment, indexes, and check
constraints; a column carries its type, nullability, default, whether it is a
primary key, whether it is unique, and the `schema.table.column` it references.

## Credentials

The connection string is used to connect and then dropped.

- Nodes are filed under `postgres/<database>/<schema>`. That path is a grouping
  key, and it carries no host, user, or password — so nothing reaches
  `graph.db`, the wiki, the site, GraphML, or a Cypher answer. A test greps the
  whole graph for the host, the port, the user, and the password and fails if
  any of them appears.
- Every message that mentions a connection goes through `redact`, which covers
  both the URL form (`postgres://user:***@host/db`) and the keyword form
  (`password=***`).
- `db scan` is CLI-only. `db_drift` is the MCP tool, because it needs no
  credentials; a connection string passed through a tool call is a credential
  written into a transcript.

TLS is offered unless the string says `sslmode=disable`, using rustls with the
webpki root store, so a hosted database works without a system OpenSSL.

## Drift

```text
1 tables in both the repository's DDL and the live catalog.
declared by DDL, absent from the server: legacy_carts
on the server, declared by no DDL here: sales.open_orders, sales.orders
```

Matching is by table name with the schema qualifier dropped, so a migration's
`CREATE TABLE orders` and the server's `sales.orders` are one table.

## Deliberate limits

- `PostgreSQL` only. The catalog queries are `pg_catalog` queries; MySQL and
  SQL Server have different ones, and pretending otherwise would mean guessing.
- A catalog is a snapshot taken when you ran `scan`. Nothing watches the
  database, and the graph does not expire it — rerun `scan` after a migration.
- Drift is name-level: a table present on both sides is "matched" even if its
  columns differ. Column-level drift needs the DDL side to be parsed into
  columns, which [artifacts](storage.md) does not do yet.
- A repository that keeps its migrations in another repository reads as
  all-live. That is a fact about the repository, not about the database, and
  the report says so.
- Row data is never read. Only the catalog is.
