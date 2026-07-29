//! Live `PostgreSQL` catalog ingestion — P1.8 of `docs/capability-coverage.md`.
//!
//! A repository's SQL files say what the schema was meant to be; the server
//! says what it is. Both belong in the graph, under the same vocabulary as
//! every other artifact: a table is a `DatabaseTable`, a column is a
//! `DatabaseColumn`, and a foreign key is a `References` edge — the same kind
//! `crate::artifacts` produces from DDL, so a query does not need to know which
//! half it is looking at.
//!
//! The declared and the observed stay apart. DDL is `Perspective::Declared`;
//! the catalog is `Perspective::Observed`. [`drift`] reports the two ways they
//! disagree instead of quietly preferring one.
//!
//! **Credentials never reach the graph.** The connection string is used to
//! connect and then dropped: nodes are filed under `postgres/<database>/<schema>`,
//! which carries no host, user, or password, and every message about a
//! connection goes through [`redact`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::json;

use crate::{
    error::{Error, Result},
    storage::{
        Confidence, Edge, EdgeKind, EvidenceKind, Graph, Node, NodeKind, Perspective, Provenance,
    },
};

/// Schemas that belong to the server rather than to the application.
const SYSTEM_SCHEMAS: &[&str] = &["pg_catalog", "information_schema", "pg_toast"];

/// One column, as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Column name.
    pub name: String,
    /// Type as the server formats it (`character varying(20)`, `numeric(10,2)`).
    pub data_type: String,
    /// Whether the column accepts nulls.
    pub nullable: bool,
    /// Default expression, when the column has one.
    pub default: Option<String>,
    /// Part of the primary key.
    pub primary_key: bool,
    /// Covered by a unique constraint or unique index.
    pub unique: bool,
    /// `schema.table.column` this column points at, when it is a foreign key.
    pub references: Option<String>,
}

/// One index, as the catalog describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    /// Index name.
    pub name: String,
    /// Columns it covers, in order.
    pub columns: Vec<String>,
    /// Whether it enforces uniqueness.
    pub unique: bool,
    /// Whether it backs the primary key.
    pub primary: bool,
}

/// One table or view, with everything the catalog says about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    /// Owning schema.
    pub schema: String,
    /// Table or view name.
    pub name: String,
    /// `table`, `view`, `materialized view`, `partitioned table`, or `foreign table`.
    pub kind: String,
    /// `COMMENT ON TABLE`, when one is set.
    pub comment: Option<String>,
    /// Columns in ordinal order.
    pub columns: Vec<Column>,
    /// Indexes on this table.
    pub indexes: Vec<Index>,
    /// `CHECK` constraint definitions, as the server prints them.
    pub checks: Vec<String>,
    /// `(local columns, target `schema.table`, target columns)` per foreign key.
    pub foreign_keys: Vec<(Vec<String>, String, Vec<String>)>,
}

impl Table {
    /// `schema.table` — the name everything else refers to it by.
    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}.{}", self.schema, self.name)
    }
}

/// One database's catalog.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Catalog {
    /// Database name, which is the only part of the connection worth keeping.
    pub database: String,
    /// Tables and views, ordered by schema then name.
    pub tables: Vec<Table>,
}

/// Hides the password in a connection string so it can be printed.
///
/// A graph server logs what it does, and a logged connection string is a leaked
/// password. Both URL form (`postgres://user:pw@host/db`) and keyword form
/// (`password=pw`) are covered, and anything unparseable is reduced to a
/// description rather than echoed.
#[must_use]
pub fn redact(url: &str) -> String {
    if url.contains("://") {
        let Some((scheme, rest)) = url.split_once("://") else {
            return "<connection string>".to_string();
        };
        let Some((authority, tail)) = rest.split_once('/') else {
            return format!("{scheme}://{}", redact_authority(rest));
        };
        return format!("{scheme}://{}/{tail}", redact_authority(authority));
    }
    url.split_whitespace()
        .map(|pair| {
            if pair.to_ascii_lowercase().starts_with("password=") {
                "password=***".to_string()
            } else {
                pair.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn redact_authority(authority: &str) -> String {
    let Some((credentials, host)) = authority.rsplit_once('@') else {
        return authority.to_string();
    };
    let user = credentials.split_once(':').map_or(credentials, |(u, _)| u);
    format!("{user}:***@{host}")
}

/// The connection string to use: the argument, or `AAG_DATABASE_URL`, or
/// `DATABASE_URL`.
///
/// # Errors
/// Returns [`Error::Protocol`] when none of the three is set, naming all three
/// rather than failing with an empty string.
pub fn connection_string(argument: &str) -> Result<String> {
    if !argument.trim().is_empty() {
        return Ok(argument.trim().to_string());
    }
    for variable in ["AAG_DATABASE_URL", "DATABASE_URL"] {
        if let Ok(value) = std::env::var(variable)
            && !value.trim().is_empty()
        {
            return Ok(value.trim().to_string());
        }
    }
    Err(Error::Protocol {
        context: "no database to connect to",
        detail: "pass --url, or set AAG_DATABASE_URL or DATABASE_URL".to_string(),
    })
}

/// Reads a live catalog over the wire.
///
/// TLS is offered unless the connection string disables it, and the server
/// decides: `sslmode=disable` connects in the clear, anything else negotiates
/// and falls back to what the server allows.
///
/// # Errors
/// Returns [`Error::Protocol`] when the connection or any catalog query fails.
/// The message names the database and never the credentials.
pub fn fetch(url: &str) -> Result<Catalog> {
    let mut client = connect(url)?;
    let database: String = client
        .query_one("SELECT current_database()", &[])
        .map_err(|error| query_failed("current database", &error))?
        .get(0);

    let mut tables = read_tables(&mut client)?;
    let mut by_key: BTreeMap<(String, String), usize> = BTreeMap::new();
    for (position, table) in tables.iter().enumerate() {
        by_key.insert((table.schema.clone(), table.name.clone()), position);
    }
    read_columns(&mut client, &mut tables, &by_key)?;
    read_constraints(&mut client, &mut tables, &by_key)?;
    read_indexes(&mut client, &mut tables, &by_key)?;
    Ok(Catalog { database, tables })
}

fn connect(url: &str) -> Result<postgres::Client> {
    if url.contains("sslmode=disable") {
        return postgres::Client::connect(url, postgres::NoTls)
            .map_err(|error| connect_failed(url, &error));
    }
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let tls = tokio_postgres_rustls::MakeRustlsConnect::new(config);
    postgres::Client::connect(url, tls).map_err(|error| connect_failed(url, &error))
}

fn connect_failed(url: &str, error: &postgres::Error) -> Error {
    Error::Protocol {
        context: "database connection failed",
        detail: format!("{}: {error}", redact(url)),
    }
}

fn query_failed(what: &'static str, error: &postgres::Error) -> Error {
    Error::Protocol {
        context: "catalog query failed",
        detail: format!("{what}: {error}"),
    }
}

/// Relation kinds worth a node: ordinary, partitioned, and foreign tables, plus
/// views and materialized views. A view is a contract something depends on, so
/// leaving it out would hide half the schema.
const RELATION_KINDS: &str = "'r', 'p', 'f', 'v', 'm'";

fn read_tables(client: &mut postgres::Client) -> Result<Vec<Table>> {
    let statement = format!(
        "SELECT n.nspname::text, c.relname::text, c.relkind::text, obj_description(c.oid)
         FROM pg_class c
         JOIN pg_namespace n ON n.oid = c.relnamespace
         WHERE c.relkind IN ({RELATION_KINDS}) AND n.nspname <> ALL($1)
         ORDER BY n.nspname, c.relname"
    );
    let rows = client
        .query(&statement, &[&SYSTEM_SCHEMAS])
        .map_err(|error| query_failed("tables", &error))?;
    Ok(rows
        .iter()
        .map(|row| Table {
            schema: row.get(0),
            name: row.get(1),
            kind: relation_kind(row.get::<_, String>(2).as_str()).to_string(),
            comment: row.get(3),
            columns: Vec::new(),
            indexes: Vec::new(),
            checks: Vec::new(),
            foreign_keys: Vec::new(),
        })
        .collect())
}

const fn relation_kind(code: &str) -> &'static str {
    match code.as_bytes() {
        b"p" => "partitioned table",
        b"f" => "foreign table",
        b"v" => "view",
        b"m" => "materialized view",
        _ => "table",
    }
}

fn read_columns(
    client: &mut postgres::Client,
    tables: &mut [Table],
    by_key: &BTreeMap<(String, String), usize>,
) -> Result<()> {
    let statement = format!(
        "SELECT n.nspname::text, c.relname::text, a.attname::text,
                format_type(a.atttypid, a.atttypmod), a.attnotnull,
                pg_get_expr(d.adbin, d.adrelid)
         FROM pg_attribute a
         JOIN pg_class c ON c.oid = a.attrelid
         JOIN pg_namespace n ON n.oid = c.relnamespace
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attnum > 0 AND NOT a.attisdropped
           AND c.relkind IN ({RELATION_KINDS}) AND n.nspname <> ALL($1)
         ORDER BY n.nspname, c.relname, a.attnum"
    );
    let rows = client
        .query(&statement, &[&SYSTEM_SCHEMAS])
        .map_err(|error| query_failed("columns", &error))?;
    for row in &rows {
        let key: (String, String) = (row.get(0), row.get(1));
        let Some(table) = by_key.get(&key).and_then(|index| tables.get_mut(*index)) else {
            continue;
        };
        table.columns.push(Column {
            name: row.get(2),
            data_type: row.get(3),
            nullable: !row.get::<_, bool>(4),
            default: row.get(5),
            primary_key: false,
            unique: false,
            references: None,
        });
    }
    Ok(())
}

fn read_constraints(
    client: &mut postgres::Client,
    tables: &mut [Table],
    by_key: &BTreeMap<(String, String), usize>,
) -> Result<()> {
    // `conkey`/`confkey` are attribute-number arrays; unnesting them WITH
    // ORDINALITY is what keeps a composite key's column order intact.
    let statement = "
        SELECT n.nspname::text, c.relname::text, con.contype::text,
               pg_get_constraintdef(con.oid),
               (SELECT array_agg(a.attname::text ORDER BY k.ord)
                  FROM unnest(con.conkey) WITH ORDINALITY AS k(num, ord)
                  JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.num),
               tn.nspname::text, t.relname::text,
               (SELECT array_agg(a.attname::text ORDER BY k.ord)
                  FROM unnest(con.confkey) WITH ORDINALITY AS k(num, ord)
                  JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.num)
        FROM pg_constraint con
        JOIN pg_class c ON c.oid = con.conrelid
        JOIN pg_namespace n ON n.oid = c.relnamespace
        LEFT JOIN pg_class t ON t.oid = con.confrelid
        LEFT JOIN pg_namespace tn ON tn.oid = t.relnamespace
        WHERE n.nspname <> ALL($1)
        ORDER BY n.nspname, c.relname, con.conname";
    let rows = client
        .query(statement, &[&SYSTEM_SCHEMAS])
        .map_err(|error| query_failed("constraints", &error))?;
    for row in &rows {
        let key: (String, String) = (row.get(0), row.get(1));
        let Some(table) = by_key.get(&key).and_then(|index| tables.get_mut(*index)) else {
            continue;
        };
        let kind: String = row.get(2);
        let definition: String = row.get(3);
        let columns: Vec<String> = row.get::<_, Option<Vec<String>>>(4).unwrap_or_default();
        match kind.as_str() {
            "p" => mark(table, &columns, |column| column.primary_key = true),
            "u" => mark(table, &columns, |column| column.unique = true),
            "c" => table.checks.push(definition),
            "f" => {
                let (Some(target_schema), Some(target_name)) = (
                    row.get::<_, Option<String>>(5),
                    row.get::<_, Option<String>>(6),
                ) else {
                    continue;
                };
                let target = format!("{target_schema}.{target_name}");
                let target_columns: Vec<String> =
                    row.get::<_, Option<Vec<String>>>(7).unwrap_or_default();
                for (position, column) in columns.iter().enumerate() {
                    let Some(target_column) = target_columns.get(position) else {
                        continue;
                    };
                    let reference = format!("{target}.{target_column}");
                    mark(table, std::slice::from_ref(column), |column| {
                        column.references = Some(reference.clone());
                    });
                }
                table.foreign_keys.push((columns, target, target_columns));
            }
            _ => {}
        }
    }
    Ok(())
}

fn mark(table: &mut Table, names: &[String], mut apply: impl FnMut(&mut Column)) {
    for column in &mut table.columns {
        if names.contains(&column.name) {
            apply(column);
        }
    }
}

fn read_indexes(
    client: &mut postgres::Client,
    tables: &mut [Table],
    by_key: &BTreeMap<(String, String), usize>,
) -> Result<()> {
    let statement = "
        SELECT n.nspname::text, t.relname::text, i.relname::text,
               ix.indisunique, ix.indisprimary,
               (SELECT array_agg(a.attname::text ORDER BY k.ord)
                  FROM unnest(ix.indkey) WITH ORDINALITY AS k(num, ord)
                  JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = k.num)
        FROM pg_index ix
        JOIN pg_class i ON i.oid = ix.indexrelid
        JOIN pg_class t ON t.oid = ix.indrelid
        JOIN pg_namespace n ON n.oid = t.relnamespace
        WHERE n.nspname <> ALL($1)
        ORDER BY n.nspname, t.relname, i.relname";
    let rows = client
        .query(statement, &[&SYSTEM_SCHEMAS])
        .map_err(|error| query_failed("indexes", &error))?;
    for row in &rows {
        let key: (String, String) = (row.get(0), row.get(1));
        let Some(table) = by_key.get(&key).and_then(|index| tables.get_mut(*index)) else {
            continue;
        };
        let columns: Vec<String> = row.get::<_, Option<Vec<String>>>(5).unwrap_or_default();
        let unique: bool = row.get(3);
        if unique {
            mark(table, &columns, |column| column.unique = true);
        }
        table.indexes.push(Index {
            name: row.get(2),
            columns,
            unique,
            primary: row.get(4),
        });
    }
    Ok(())
}

/// What one ingestion changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Ingested {
    /// Tables and views written.
    pub tables: u32,
    /// Columns written.
    pub columns: u32,
    /// Edges written: containment plus foreign keys.
    pub edges: u32,
}

/// Writes a catalog into the graph.
///
/// Nodes are filed under `postgres/<database>/<schema>`, a path that exists
/// only as a grouping key: it carries the database and the schema, and nothing
/// that could authenticate anyone.
///
/// # Errors
/// Returns a storage error when the graph cannot be written.
pub fn ingest(graph: &Graph, catalog: &Catalog) -> Result<Ingested> {
    let mut summary = Ingested::default();
    let mut table_ids: BTreeMap<String, i64> = BTreeMap::new();
    let mut column_ids: BTreeMap<String, i64> = BTreeMap::new();
    write_relations(
        graph,
        catalog,
        &mut summary,
        &mut table_ids,
        &mut column_ids,
    )?;
    // Foreign keys after every table exists, so a key pointing forward in
    // alphabetical order still finds its target.
    write_foreign_keys(graph, catalog, &mut summary, &table_ids, &column_ids)?;
    Ok(summary)
}

/// Writes every table and its columns, recording the ids the foreign-key pass
/// needs.
fn write_relations(
    graph: &Graph,
    catalog: &Catalog,
    summary: &mut Ingested,
    table_ids: &mut BTreeMap<String, i64>,
    column_ids: &mut BTreeMap<String, i64>,
) -> Result<()> {
    for table in &catalog.tables {
        let file_path = format!("postgres/{}/{}", catalog.database, table.schema);
        let provenance = Provenance {
            perspective: Perspective::Observed,
            evidence_kind: EvidenceKind::SqlSchema,
            evidence_source: Some(file_path.clone()),
        };
        let details = json!({
            "relation": table.kind,
            "comment": table.comment,
            "indexes": table.indexes.iter().map(|index| json!({
                "name": index.name,
                "columns": index.columns,
                "unique": index.unique,
                "primary": index.primary,
            })).collect::<Vec<_>>(),
            "checks": table.checks,
        });
        let qualified = table.qualified();
        let table_id = graph.insert_node_with_provenance(
            &Node {
                id: None,
                kind: NodeKind::DatabaseTable,
                name: qualified.clone(),
                file_path: file_path.clone(),
                start_line: 1,
                end_line: 1,
                description: Some(details.to_string()),
            },
            &provenance,
        )?;
        table_ids.insert(qualified.clone(), table_id);
        summary.tables = summary.tables.saturating_add(1);

        for (position, column) in table.columns.iter().enumerate() {
            let line = u32::try_from(position + 1).unwrap_or(u32::MAX);
            let details = json!({
                "type": column.data_type,
                "nullable": column.nullable,
                "default": column.default,
                "primary_key": column.primary_key,
                "unique": column.unique,
                "references": column.references,
            });
            let name = format!("{qualified}.{}", column.name);
            let column_id = graph.insert_node_with_provenance(
                &Node {
                    id: None,
                    kind: NodeKind::DatabaseColumn,
                    name: name.clone(),
                    file_path: file_path.clone(),
                    start_line: line,
                    end_line: line,
                    description: Some(details.to_string()),
                },
                &provenance,
            )?;
            column_ids.insert(name, column_id);
            summary.columns = summary.columns.saturating_add(1);
            graph.insert_edge_with_provenance(
                &Edge {
                    src: table_id,
                    dst: column_id,
                    kind: EdgeKind::References,
                    confidence: Confidence::Extracted,
                },
                &provenance,
            )?;
            summary.edges = summary.edges.saturating_add(1);
        }
    }

    Ok(())
}

/// Links each foreign key twice: table to table, and column to column.
fn write_foreign_keys(
    graph: &Graph,
    catalog: &Catalog,
    summary: &mut Ingested,
    table_ids: &BTreeMap<String, i64>,
    column_ids: &BTreeMap<String, i64>,
) -> Result<()> {
    for table in &catalog.tables {
        let file_path = format!("postgres/{}/{}", catalog.database, table.schema);
        let provenance = Provenance {
            perspective: Perspective::Observed,
            evidence_kind: EvidenceKind::SqlSchema,
            evidence_source: Some(file_path),
        };
        let Some(&source) = table_ids.get(&table.qualified()) else {
            continue;
        };
        for (columns, target, target_columns) in &table.foreign_keys {
            if let Some(&target_id) = table_ids.get(target) {
                graph.insert_edge_with_provenance(
                    &Edge {
                        src: source,
                        dst: target_id,
                        kind: EdgeKind::References,
                        confidence: Confidence::Extracted,
                    },
                    &provenance,
                )?;
                summary.edges = summary.edges.saturating_add(1);
            }
            for (position, column) in columns.iter().enumerate() {
                let Some(target_column) = target_columns.get(position) else {
                    continue;
                };
                let from = format!("{}.{column}", table.qualified());
                let to = format!("{target}.{target_column}");
                let (Some(&src), Some(&dst)) = (column_ids.get(&from), column_ids.get(&to)) else {
                    continue;
                };
                graph.insert_edge_with_provenance(
                    &Edge {
                        src,
                        dst,
                        kind: EdgeKind::References,
                        confidence: Confidence::Extracted,
                    },
                    &provenance,
                )?;
                summary.edges = summary.edges.saturating_add(1);
            }
        }
    }
    Ok(())
}

/// Connects, reads, ingests, and reports — the whole of `aag db scan`.
///
/// # Errors
/// As [`fetch`] and [`ingest`], plus [`Error::IndexMissing`] when `root` has no
/// graph to write into.
pub fn scan(root: &Path, url: &str) -> Result<String> {
    let url = connection_string(url)?;
    let catalog = fetch(&url)?;
    let graph = Graph::open_existing(root)?;
    let summary = ingest(&graph, &catalog)?;
    let schemas: BTreeSet<&str> = catalog
        .tables
        .iter()
        .map(|table| table.schema.as_str())
        .collect();
    Ok(format!(
        "ingested {} tables and views, {} columns, {} relations from `{}` \
         ({} schema{}: {}).\nThe connection string is not stored: nodes are filed under \
         postgres/{}/<schema>.",
        summary.tables,
        summary.columns,
        summary.edges,
        catalog.database,
        schemas.len(),
        if schemas.len() == 1 { "" } else { "s" },
        schemas.into_iter().collect::<Vec<_>>().join(", "),
        catalog.database,
    ))
}

/// The two ways a live schema and the repository's DDL disagree.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Drift {
    /// Declared by a `.sql` file, absent from the server.
    pub declared_only: Vec<String>,
    /// On the server, declared by no `.sql` file in this repository.
    pub live_only: Vec<String>,
    /// In both, matched by name.
    pub matched: Vec<String>,
}

/// Compares declared DDL tables with ingested catalog tables.
///
/// Matching is by table name, with the schema qualifier dropped from the live
/// side: a `CREATE TABLE orders` in a migration and `sales.orders` on the
/// server are the same table, and treating them as two would make the report
/// noise.
///
/// # Errors
/// Returns [`Error::IndexMissing`] when `root` has no graph.
pub fn drift(root: &Path) -> Result<Drift> {
    let graph = Graph::open_existing(root)?;
    let nodes = graph.all_nodes_with_provenance()?;
    let mut declared: BTreeMap<String, String> = BTreeMap::new();
    let mut live: BTreeMap<String, String> = BTreeMap::new();
    for (node, provenance) in &nodes {
        if node.kind != NodeKind::DatabaseTable {
            continue;
        }
        let bare = node
            .name
            .rsplit('.')
            .next()
            .unwrap_or(&node.name)
            .to_ascii_lowercase();
        if provenance.perspective == Perspective::Declared {
            declared.insert(bare, node.name.clone());
        } else {
            live.insert(bare, node.name.clone());
        }
    }
    let mut report = Drift::default();
    for (bare, name) in &declared {
        if live.contains_key(bare) {
            report.matched.push(name.clone());
        } else {
            report.declared_only.push(name.clone());
        }
    }
    for (bare, name) in &live {
        if !declared.contains_key(bare) {
            report.live_only.push(name.clone());
        }
    }
    Ok(report)
}

/// The drift report as text.
///
/// # Errors
/// As [`drift`].
pub fn format_drift(root: &Path) -> Result<String> {
    let report = drift(root)?;
    if report.matched.is_empty() && report.declared_only.is_empty() && report.live_only.is_empty() {
        return Ok(
            "no database tables in the graph. `aag db scan --url <url>` ingests a live \
                   catalog; DDL in `.sql` files is indexed automatically."
                .to_string(),
        );
    }
    let mut lines = vec![format!(
        "{} tables in both the repository's DDL and the live catalog.",
        report.matched.len()
    )];
    if !report.declared_only.is_empty() {
        lines.push(format!(
            "declared by DDL, absent from the server: {}",
            report.declared_only.join(", ")
        ));
    }
    if !report.live_only.is_empty() {
        lines.push(format!(
            "on the server, declared by no DDL here: {}",
            report.live_only.join(", ")
        ));
    }
    lines.push(
        "Matching is by table name, ignoring the schema qualifier. A repository that keeps its \
         migrations elsewhere will read as all-live, which is a fact about this repository \
         rather than about the database."
            .to_string(),
    );
    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog() -> Catalog {
        Catalog {
            database: "shop".to_string(),
            tables: vec![
                Table {
                    schema: "public".to_string(),
                    name: "customers".to_string(),
                    kind: "table".to_string(),
                    comment: None,
                    columns: vec![Column {
                        name: "id".to_string(),
                        data_type: "integer".to_string(),
                        nullable: false,
                        default: Some("nextval('customers_id_seq')".to_string()),
                        primary_key: true,
                        unique: true,
                        references: None,
                    }],
                    indexes: vec![Index {
                        name: "customers_pkey".to_string(),
                        columns: vec!["id".to_string()],
                        unique: true,
                        primary: true,
                    }],
                    checks: Vec::new(),
                    foreign_keys: Vec::new(),
                },
                Table {
                    schema: "sales".to_string(),
                    name: "orders".to_string(),
                    kind: "table".to_string(),
                    comment: Some("one row per placed order".to_string()),
                    columns: vec![Column {
                        name: "customer_id".to_string(),
                        data_type: "integer".to_string(),
                        nullable: false,
                        default: None,
                        primary_key: false,
                        unique: false,
                        references: Some("public.customers.id".to_string()),
                    }],
                    indexes: Vec::new(),
                    checks: vec!["CHECK ((total >= (0)::numeric))".to_string()],
                    foreign_keys: vec![(
                        vec!["customer_id".to_string()],
                        "public.customers".to_string(),
                        vec!["id".to_string()],
                    )],
                },
            ],
        }
    }

    #[test]
    fn a_password_never_survives_being_printed() {
        assert_eq!(
            redact("postgres://app:hunter2@db.internal:5432/shop"),
            "postgres://app:***@db.internal:5432/shop"
        );
        assert_eq!(
            redact("host=db.internal user=app password=hunter2 dbname=shop"),
            "host=db.internal user=app password=*** dbname=shop"
        );
        assert!(
            !redact("postgresql://app:hunter2@db/shop?sslmode=require").contains("hunter2"),
            "a query string does not smuggle it out either"
        );
        assert_eq!(
            redact("postgres://db.internal/shop"),
            "postgres://db.internal/shop",
            "a connection with no credentials is left readable"
        );
    }

    #[test]
    fn the_catalog_becomes_tables_columns_and_foreign_keys() {
        let graph = Graph::open_in_memory().unwrap();

        let summary = ingest(&graph, &catalog()).unwrap();

        assert_eq!(summary.tables, 2);
        assert_eq!(summary.columns, 2);
        // Two containment edges, one table-to-table key, one column-to-column.
        assert_eq!(summary.edges, 4);
        let orders = graph.find_by_name("sales.orders").unwrap().unwrap();
        assert_eq!(orders.kind, NodeKind::DatabaseTable);
        assert_eq!(
            orders.file_path, "postgres/shop/sales",
            "the path carries the database and schema, and nothing else"
        );
        let column = graph
            .find_by_name("sales.orders.customer_id")
            .unwrap()
            .unwrap();
        assert_eq!(column.kind, NodeKind::DatabaseColumn);
        let details = column.description.unwrap();
        assert!(
            details.contains("\"references\":\"public.customers.id\""),
            "{details}"
        );
        assert!(details.contains("\"nullable\":false"), "{details}");
        let customers = graph.find_by_name("public.customers").unwrap().unwrap();
        assert!(
            graph
                .callers(customers.id.unwrap())
                .unwrap()
                .iter()
                .any(|(node, kind, _)| node.name == "sales.orders"
                    && *kind == EdgeKind::References),
            "the foreign key is an edge between the tables"
        );
    }

    #[test]
    fn no_part_of_a_connection_string_reaches_the_graph() {
        let graph = Graph::open_in_memory().unwrap();

        ingest(&graph, &catalog()).unwrap();

        let mut dumped = String::new();
        for node in &graph.all_nodes().unwrap() {
            dumped.push_str(&node.name);
            dumped.push_str(&node.file_path);
            dumped.push_str(node.description.as_deref().unwrap_or_default());
        }
        for secret in ["hunter2", "db.internal", "5432", "app:"] {
            assert!(!dumped.contains(secret), "`{secret}` leaked into the graph");
        }
    }

    #[test]
    fn drift_reports_both_directions_and_matches_across_the_schema_qualifier() {
        let root = std::env::temp_dir().join(format!("aag-db-drift-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("schema.sql"),
            "CREATE TABLE customers (id INT); CREATE TABLE archived (id INT);",
        )
        .unwrap();
        crate::bigbang::run(
            &root,
            &crate::bigbang::Options {
                no_viz: true,
                no_install: true,
                ..Default::default()
            },
        )
        .unwrap();
        let graph = Graph::open_existing(&root).unwrap();
        ingest(&graph, &catalog()).unwrap();
        drop(graph);

        let report = drift(&root).unwrap();

        assert_eq!(
            report.matched,
            vec!["customers".to_string()],
            "`customers` and `public.customers` are one table"
        );
        assert_eq!(report.declared_only, vec!["archived".to_string()]);
        assert_eq!(report.live_only, vec!["sales.orders".to_string()]);
    }

    #[test]
    fn a_missing_connection_string_names_every_way_to_supply_one() {
        // Set by the environment this runs in, not by the test.
        if std::env::var("AAG_DATABASE_URL").is_ok() || std::env::var("DATABASE_URL").is_ok() {
            return;
        }
        let message = connection_string("  ").unwrap_err().to_string();
        assert!(message.contains("--url"), "{message}");
        assert!(message.contains("AAG_DATABASE_URL"), "{message}");
        assert!(message.contains("DATABASE_URL"), "{message}");
    }

    /// Runs only when a server is pointed at: `AAG_TEST_POSTGRES_URL=… cargo test`.
    /// Everything above it is hermetic, and this is the one that proves the
    /// catalog queries are real SQL rather than plausible SQL.
    #[test]
    fn a_live_catalog_round_trips() {
        let Ok(url) = std::env::var("AAG_TEST_POSTGRES_URL") else {
            return;
        };
        let catalog = fetch(&url).expect("read the catalog");
        let orders = catalog
            .tables
            .iter()
            .find(|table| table.qualified() == "sales.orders")
            .expect("sales.orders");
        assert_eq!(orders.comment.as_deref(), Some("one row per placed order"));
        assert!(
            orders
                .columns
                .iter()
                .any(|column| column.name == "id" && column.primary_key),
            "{:?}",
            orders.columns
        );
        assert!(
            orders
                .columns
                .iter()
                .any(|column| column.name == "customer_id"
                    && column.references.as_deref() == Some("public.customers.id")),
            "{:?}",
            orders.columns
        );
        assert!(
            orders
                .columns
                .iter()
                .any(|column| column.name == "note" && column.nullable),
            "nullability is read, not assumed: {:?}",
            orders.columns
        );
        assert!(
            orders
                .indexes
                .iter()
                .any(|index| index.name == "orders_customer_idx"),
            "{:?}",
            orders.indexes
        );
        assert!(
            orders.checks.iter().any(|check| check.contains("total")),
            "{:?}",
            orders.checks
        );
        assert!(
            catalog
                .tables
                .iter()
                .any(|table| table.qualified() == "sales.open_orders" && table.kind == "view"),
            "a view is part of the schema"
        );
        assert!(
            catalog
                .tables
                .iter()
                .all(|table| !SYSTEM_SCHEMAS.contains(&table.schema.as_str())),
            "the server's own catalog is not the application's schema"
        );
    }
}
