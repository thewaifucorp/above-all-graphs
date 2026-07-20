//! `SQLite`-backed graph storage: nodes (symbols/files) and edges (relations).
//!
//! Every edge carries a confidence tag — `EXTRACTED` (explicit in source),
//! `INFERRED` (resolved by heuristic), or `AMBIGUOUS` (not resolved with
//! certainty) — per `SPEC.md` section 3. This is the single source of truth
//! the parser (writer) and CLI/MCP surface (reader) both sit on top of.

use std::path::Path;

use rusqlite::Connection;

use crate::error::{Error, Result};

/// Whether a graph fact was asserted by a contract/human source or observed
/// from implementation/runtime evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perspective {
    /// Intended behavior from documentation, contracts, or maintainers.
    Declared,
    /// Implementation or runtime behavior demonstrated by evidence.
    Observed,
}

impl Perspective {
    /// Stable string form stored in `SQLite` and exported by the protocol compiler.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declared => "declared",
            Self::Observed => "observed",
        }
    }

    fn from_str(value: &str) -> Self {
        if value == "declared" {
            Self::Declared
        } else {
            Self::Observed
        }
    }
}

/// Machine-readable origin of a graph fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceKind {
    /// Definition extracted from an AST.
    AstDefinition,
    /// Import extracted from an AST.
    AstImport,
    /// Call extracted or resolved from an AST.
    AstCall,
    /// Inheritance or implementation extracted from an AST.
    AstInheritance,
    /// Text or documentation reference.
    TextReference,
    /// `OpenAPI` or Swagger contract declaration.
    OpenApi,
    /// SQL DDL schema declaration.
    SqlSchema,
    /// Infrastructure-as-code declaration.
    Infrastructure,
    /// Relationship inferred when no stronger source is available.
    InferredRelationship,
}

impl EvidenceKind {
    /// Stable value aligned with the AAG Protocol evidence vocabulary.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::AstDefinition => "ast_definition",
            Self::AstImport => "ast_import",
            Self::AstCall => "ast_call",
            Self::AstInheritance => "ast_inheritance",
            Self::TextReference => "text_reference",
            Self::OpenApi => "openapi_contract",
            Self::SqlSchema => "sql_schema",
            Self::Infrastructure => "infrastructure_definition",
            Self::InferredRelationship => "inferred_relationship",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "ast_import" => Self::AstImport,
            "ast_call" => Self::AstCall,
            "ast_inheritance" => Self::AstInheritance,
            "text_reference" => Self::TextReference,
            "openapi_contract" => Self::OpenApi,
            "sql_schema" => Self::SqlSchema,
            "infrastructure_definition" => Self::Infrastructure,
            "inferred_relationship" => Self::InferredRelationship,
            _ => Self::AstDefinition,
        }
    }
}

/// Provenance attached to a node or edge independently of its graph shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// Declared intent or observed implementation.
    pub perspective: Perspective,
    /// Evidence mechanism supporting the fact.
    pub evidence_kind: EvidenceKind,
    /// Optional repository-relative evidence source override.
    pub evidence_source: Option<String>,
}

impl Provenance {
    /// Provenance for source code or text documents indexed by the default pipeline.
    #[must_use]
    pub fn for_node(node: &Node) -> Self {
        if matches!(
            node.kind,
            NodeKind::Doc | NodeKind::Endpoint | NodeKind::Schema
        ) {
            Self {
                perspective: Perspective::Declared,
                evidence_kind: EvidenceKind::TextReference,
                evidence_source: Some(node.file_path.clone()),
            }
        } else {
            Self {
                perspective: Perspective::Observed,
                evidence_kind: EvidenceKind::AstDefinition,
                evidence_source: Some(node.file_path.clone()),
            }
        }
    }

    /// Provenance for a resolved graph relationship.
    #[must_use]
    pub const fn for_edge(edge: &Edge) -> Self {
        let (perspective, evidence_kind) = match (edge.kind, edge.confidence) {
            (EdgeKind::Explains, _) => (Perspective::Declared, EvidenceKind::TextReference),
            (EdgeKind::References, _) => (Perspective::Declared, EvidenceKind::OpenApi),
            (_, Confidence::Ambiguous) => {
                (Perspective::Observed, EvidenceKind::InferredRelationship)
            }
            (EdgeKind::Imports, _) => (Perspective::Observed, EvidenceKind::AstImport),
            (EdgeKind::Calls, _) => (Perspective::Observed, EvidenceKind::AstCall),
            (EdgeKind::Inherits | EdgeKind::Implements, _) => {
                (Perspective::Observed, EvidenceKind::AstInheritance)
            }
        };
        Self {
            perspective,
            evidence_kind,
            evidence_source: None,
        }
    }
}

/// How confident the graph is that an edge reflects reality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confidence {
    /// Explicit in source (e.g. a direct `use` import).
    Extracted,
    /// Resolved by heuristic (e.g. `self` receiver type inference).
    Inferred,
    /// Could not be resolved with certainty (e.g. dynamic dispatch).
    Ambiguous,
}

impl Confidence {
    /// Stable string form stored in `SQLite` and shown to agents.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Extracted => "EXTRACTED",
            Self::Inferred => "INFERRED",
            Self::Ambiguous => "AMBIGUOUS",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "EXTRACTED" => Self::Extracted,
            "INFERRED" => Self::Inferred,
            _ => Self::Ambiguous,
        }
    }
}

/// The kind of symbol a node represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    /// A source file, tracked so edges can point at "the file" itself.
    File,
    /// A free function.
    Function,
    /// A struct, class, or equivalent product type.
    Struct,
    /// A method on a struct/impl/class.
    Method,
    /// An interface/trait.
    Interface,
    /// A doc/PDF/image — multimodal content linked to the code it explains.
    /// See `crate::resolve` (text docs, indexed immediately) and
    /// `crate::docs` (binary docs, described later by the host agent).
    Doc,
    /// An HTTP operation declared by an API contract.
    Endpoint,
    /// A reusable data schema declared by an API contract.
    Schema,
    /// A database table declared by DDL.
    DatabaseTable,
    /// A Terraform or other infrastructure-as-code resource.
    InfraResource,
}

impl NodeKind {
    /// Stable string form stored in `SQLite`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
            Self::Function => "function",
            Self::Struct => "struct",
            Self::Method => "method",
            Self::Interface => "interface",
            Self::Doc => "doc",
            Self::Endpoint => "endpoint",
            Self::Schema => "schema",
            Self::DatabaseTable => "database_table",
            Self::InfraResource => "infra_resource",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "file" => Self::File,
            "function" => Self::Function,
            "struct" => Self::Struct,
            "method" => Self::Method,
            "doc" => Self::Doc,
            "endpoint" => Self::Endpoint,
            "schema" => Self::Schema,
            "database_table" => Self::DatabaseTable,
            "infra_resource" => Self::InfraResource,
            _ => Self::Interface,
        }
    }
}

/// A symbol or file in the graph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// Row id, `None` until inserted.
    pub id: Option<i64>,
    /// What kind of symbol this is.
    pub kind: NodeKind,
    /// Symbol name (or file path, for `NodeKind::File`).
    pub name: String,
    /// Path to the file this node lives in, relative to the indexed root.
    pub file_path: String,
    /// 1-based line where the symbol starts.
    pub start_line: u32,
    /// 1-based line where the symbol ends.
    pub end_line: u32,
    /// For `NodeKind::Doc`: the doc's text (full content for text docs, or
    /// the host agent's vision-pass description for binary docs). `None`
    /// for code nodes, and for binary docs not yet described.
    pub description: Option<String>,
}

/// The kind of relation an edge represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeKind {
    /// One file imports another symbol/file.
    Imports,
    /// One symbol calls another.
    Calls,
    /// One type inherits/extends another.
    Inherits,
    /// One type implements an interface/trait.
    Implements,
    /// A doc explains a symbol (its text mentions the symbol by name).
    Explains,
    /// A contract operation or schema references another contract schema.
    References,
}

impl EdgeKind {
    /// Stable string form stored in `SQLite`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Imports => "imports",
            Self::Calls => "calls",
            Self::Inherits => "inherits",
            Self::Implements => "implements",
            Self::Explains => "explains",
            Self::References => "references",
        }
    }

    fn from_str(value: &str) -> Self {
        match value {
            "imports" => Self::Imports,
            "inherits" => Self::Inherits,
            "implements" => Self::Implements,
            "explains" => Self::Explains,
            "references" => Self::References,
            _ => Self::Calls,
        }
    }
}

/// A relation between two nodes, tagged with how confident the resolution is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edge {
    /// Source node id.
    pub src: i64,
    /// Destination node id.
    pub dst: i64,
    /// What kind of relation this is.
    pub kind: EdgeKind,
    /// How confident the graph is that this edge is correct.
    pub confidence: Confidence,
}

/// Handle to the on-disk graph database (`.aag/graph.db`).
pub struct Graph {
    conn: Connection,
}

impl Graph {
    /// Opens (creating if absent) the graph database at `path`, applying
    /// the schema migration if needed.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the database cannot be opened or the
    /// schema cannot be created.
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).map_err(|source| Error::Storage {
            context: "open graph database",
            source,
        })?;
        let graph = Self { conn };
        graph.migrate()?;
        Ok(graph)
    }

    /// Opens the `.aag/graph.db` under `root`, requiring it to already exist.
    ///
    /// # Errors
    ///
    /// Returns [`Error::IndexMissing`] if `root` has no `.aag/graph.db` yet
    /// (callers should be told to run `aag bigbang`), or [`Error::Storage`]
    /// if the existing database cannot be opened.
    pub fn open_existing(root: &Path) -> Result<Self> {
        let db_path = root.join(".aag").join("graph.db");
        if !db_path.is_file() {
            return Err(Error::IndexMissing {
                path: root.to_path_buf(),
            });
        }
        Self::open(&db_path)
    }

    /// Opens an in-memory graph database. Used by tests and by callers that
    /// only need a throwaway graph for the lifetime of one process.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the schema cannot be created.
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().map_err(|source| Error::Storage {
            context: "open in-memory graph database",
            source,
        })?;
        let graph = Self { conn };
        graph.migrate()?;
        Ok(graph)
    }

    fn migrate(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "
                CREATE TABLE IF NOT EXISTS nodes (
                    id          INTEGER PRIMARY KEY,
                    kind        TEXT NOT NULL,
                    name        TEXT NOT NULL,
                    file_path   TEXT NOT NULL,
                    start_line  INTEGER NOT NULL,
                    end_line    INTEGER NOT NULL,
                    description TEXT
                );

                CREATE TABLE IF NOT EXISTS edges (
                    src        INTEGER NOT NULL REFERENCES nodes(id),
                    dst        INTEGER NOT NULL REFERENCES nodes(id),
                    kind       TEXT NOT NULL,
                    confidence TEXT NOT NULL,
                    PRIMARY KEY (src, dst, kind)
                );

                CREATE INDEX IF NOT EXISTS idx_nodes_name ON nodes(name);
                CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst);

                CREATE VIRTUAL TABLE IF NOT EXISTS nodes_fts USING fts5(
                    name,
                    description,
                    content='nodes',
                    content_rowid='id'
                );

                CREATE TRIGGER IF NOT EXISTS nodes_ai AFTER INSERT ON nodes BEGIN
                    INSERT INTO nodes_fts(rowid, name, description) VALUES (new.id, new.name, new.description);
                END;
                CREATE TRIGGER IF NOT EXISTS nodes_ad AFTER DELETE ON nodes BEGIN
                    INSERT INTO nodes_fts(nodes_fts, rowid, name, description) VALUES ('delete', old.id, old.name, old.description);
                END;
                CREATE TRIGGER IF NOT EXISTS nodes_au AFTER UPDATE ON nodes BEGIN
                    INSERT INTO nodes_fts(nodes_fts, rowid, name, description) VALUES ('delete', old.id, old.name, old.description);
                    INSERT INTO nodes_fts(rowid, name, description) VALUES (new.id, new.name, new.description);
                END;
                ",
            )
            .map_err(|source| Error::Storage {
                context: "apply schema migration",
                source,
            })?;
        self.ensure_column("nodes", "perspective", "TEXT NOT NULL DEFAULT 'observed'")?;
        self.ensure_column(
            "nodes",
            "evidence_kind",
            "TEXT NOT NULL DEFAULT 'ast_definition'",
        )?;
        self.ensure_column("nodes", "evidence_source", "TEXT")?;
        self.ensure_column("edges", "perspective", "TEXT NOT NULL DEFAULT 'observed'")?;
        self.ensure_column(
            "edges",
            "evidence_kind",
            "TEXT NOT NULL DEFAULT 'inferred_relationship'",
        )?;
        self.ensure_column("edges", "evidence_source", "TEXT")
    }

    fn ensure_column(&self, table: &'static str, column: &str, definition: &str) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(|source| Error::Storage {
                context: "inspect schema migration",
                source,
            })?;
        let rows = stmt
            .query_map((), |row| row.get::<_, String>(1))
            .map_err(|source| Error::Storage {
                context: "read schema migration",
                source,
            })?;
        for name in rows {
            if name.map_err(|source| Error::Storage {
                context: "read schema column",
                source,
            })? == column
            {
                return Ok(());
            }
        }
        self.conn
            .execute_batch(&format!(
                "ALTER TABLE {table} ADD COLUMN {column} {definition}"
            ))
            .map_err(|source| Error::Storage {
                context: "add provenance column",
                source,
            })
    }

    /// Runs `f` inside one `SQLite` transaction: every write it performs
    /// commits as a single fsync instead of one per statement (`rusqlite`
    /// autocommits each unwrapped `execute`/`execute_batch` on its own).
    /// `resolve::index_repo` wraps its whole clear+insert+resolve pass in
    /// this — unbatched, indexing even a few dozen tiny files took over a
    /// second, dominated entirely by rollback-journal fsyncs. Rolls back
    /// on error.
    ///
    /// # Errors
    ///
    /// Returns whatever error `f` returns, or [`Error::Storage`] if
    /// `BEGIN`/`COMMIT` itself fails.
    pub fn transaction<T>(&self, f: impl FnOnce() -> Result<T>) -> Result<T> {
        self.conn
            .execute_batch("BEGIN")
            .map_err(|source| Error::Storage {
                context: "begin transaction",
                source,
            })?;
        match f() {
            Ok(value) => {
                self.conn
                    .execute_batch("COMMIT")
                    .map_err(|source| Error::Storage {
                        context: "commit transaction",
                        source,
                    })?;
                Ok(value)
            }
            Err(error) => {
                let _ = self.conn.execute_batch("ROLLBACK");
                Err(error)
            }
        }
    }

    /// Wipes all nodes and edges, ready for a full reindex. Used by
    /// `resolve::index_repo` so re-running it (e.g. from the watcher) never
    /// accumulates stale nodes from a previous pass.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the delete fails.
    pub fn clear(&self) -> Result<()> {
        self.conn
            .execute_batch("DELETE FROM edges; DELETE FROM nodes;")
            .map_err(|source| Error::Storage {
                context: "clear graph before reindex",
                source,
            })
    }

    /// Inserts a node and returns its assigned id.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the insert fails.
    pub fn insert_node(&self, node: &Node) -> Result<i64> {
        self.insert_node_with_provenance(node, &Provenance::for_node(node))
    }

    /// Inserts a node with explicit evidence provenance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the insert fails.
    pub fn insert_node_with_provenance(&self, node: &Node, provenance: &Provenance) -> Result<i64> {
        self.conn
            .execute(
                "INSERT INTO nodes (kind, name, file_path, start_line, end_line, description,
                                    perspective, evidence_kind, evidence_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                (
                    node.kind.as_str(),
                    &node.name,
                    &node.file_path,
                    node.start_line,
                    node.end_line,
                    &node.description,
                    provenance.perspective.as_str(),
                    provenance.evidence_kind.as_str(),
                    &provenance.evidence_source,
                ),
            )
            .map_err(|source| Error::Storage {
                context: "insert node",
                source,
            })?;
        Ok(self.conn.last_insert_rowid())
    }

    /// Sets (or replaces) a node's description — the host agent's
    /// vision-pass result for a binary doc, or a later re-describe. Keeps
    /// `nodes_fts` in sync so the new text becomes searchable immediately.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the update fails.
    pub fn set_description(&self, node_id: i64, description: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE nodes SET description = ?1 WHERE id = ?2",
                (description, node_id),
            )
            .map_err(|source| Error::Storage {
                context: "set node description",
                source,
            })?;
        Ok(())
    }

    /// Inserts an edge between two existing nodes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the insert fails.
    pub fn insert_edge(&self, edge: &Edge) -> Result<()> {
        self.insert_edge_with_provenance(edge, &Provenance::for_edge(edge))
    }

    /// Inserts an edge with explicit evidence provenance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the insert fails.
    pub fn insert_edge_with_provenance(&self, edge: &Edge, provenance: &Provenance) -> Result<()> {
        self.conn
            .execute(
                "INSERT OR REPLACE INTO edges
                    (src, dst, kind, confidence, perspective, evidence_kind, evidence_source)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                (
                    edge.src,
                    edge.dst,
                    edge.kind.as_str(),
                    edge.confidence.as_str(),
                    provenance.perspective.as_str(),
                    provenance.evidence_kind.as_str(),
                    &provenance.evidence_source,
                ),
            )
            .map_err(|source| Error::Storage {
                context: "insert edge",
                source,
            })?;
        Ok(())
    }

    /// Full-text search over node names, ranked by FTS5 relevance.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<Node>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT n.id, n.kind, n.name, n.file_path, n.start_line, n.end_line, n.description
                 FROM nodes_fts f
                 JOIN nodes n ON n.id = f.rowid
                 WHERE nodes_fts MATCH ?1
                 ORDER BY rank
                 LIMIT ?2",
            )
            .map_err(|source| Error::Storage {
                context: "prepare search query",
                source,
            })?;

        let rows = stmt
            .query_map((query, limit), Self::row_to_node)
            .map_err(|source| Error::Storage {
                context: "run search query",
                source,
            })?;

        Self::collect_nodes(rows)
    }

    /// Direct callers/importers of `node_id` — edges where `dst == node_id`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn callers(&self, node_id: i64) -> Result<Vec<(Node, EdgeKind, Confidence)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT n.id, n.kind, n.name, n.file_path, n.start_line, n.end_line, n.description,
                        e.kind, e.confidence
                 FROM edges e
                 JOIN nodes n ON n.id = e.src
                 WHERE e.dst = ?1",
            )
            .map_err(|source| Error::Storage {
                context: "prepare callers query",
                source,
            })?;

        let rows = stmt
            .query_map((node_id,), |row| {
                let node = Self::row_to_node(row)?;
                let kind = EdgeKind::from_str(&row.get::<_, String>(7)?);
                let confidence = Confidence::from_str(&row.get::<_, String>(8)?);
                Ok((node, kind, confidence))
            })
            .map_err(|source| Error::Storage {
                context: "run callers query",
                source,
            })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read callers row",
                source,
            })?);
        }
        Ok(out)
    }

    /// What `node_id` directly calls/imports — edges where `src == node_id`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn callees(&self, node_id: i64) -> Result<Vec<(Node, EdgeKind, Confidence)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT n.id, n.kind, n.name, n.file_path, n.start_line, n.end_line, n.description,
                        e.kind, e.confidence
                 FROM edges e
                 JOIN nodes n ON n.id = e.dst
                 WHERE e.src = ?1",
            )
            .map_err(|source| Error::Storage {
                context: "prepare callees query",
                source,
            })?;

        let rows = stmt
            .query_map((node_id,), |row| {
                let node = Self::row_to_node(row)?;
                let kind = EdgeKind::from_str(&row.get::<_, String>(7)?);
                let confidence = Confidence::from_str(&row.get::<_, String>(8)?);
                Ok((node, kind, confidence))
            })
            .map_err(|source| Error::Storage {
                context: "run callees query",
                source,
            })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read callees row",
                source,
            })?);
        }
        Ok(out)
    }

    /// All nodes in the graph. Used by `crate::export` to build side-outputs
    /// (`graph.json`, `GRAPH_REPORT.md`, `graph.html`, ...) from one pass
    /// over the whole graph rather than many targeted queries.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn all_nodes(&self) -> Result<Vec<Node>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, name, file_path, start_line, end_line, description FROM nodes",
            )
            .map_err(|source| Error::Storage {
                context: "prepare all_nodes query",
                source,
            })?;
        let rows = stmt
            .query_map((), Self::row_to_node)
            .map_err(|source| Error::Storage {
                context: "run all_nodes query",
                source,
            })?;
        Self::collect_nodes(rows)
    }

    /// All nodes paired with their stored evidence provenance.
    ///
    /// # Errors
    /// Returns [`Error::Storage`] if the query fails.
    pub fn all_nodes_with_provenance(&self) -> Result<Vec<(Node, Provenance)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, kind, name, file_path, start_line, end_line, description,
                    perspective, evidence_kind, evidence_source FROM nodes",
            )
            .map_err(|source| Error::Storage {
                context: "prepare provenance node query",
                source,
            })?;
        let rows = stmt
            .query_map((), |row| {
                Ok((
                    Self::row_to_node(row)?,
                    Provenance {
                        perspective: Perspective::from_str(&row.get::<_, String>(7)?),
                        evidence_kind: EvidenceKind::from_str(&row.get::<_, String>(8)?),
                        evidence_source: row.get(9)?,
                    },
                ))
            })
            .map_err(|source| Error::Storage {
                context: "run provenance node query",
                source,
            })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read provenance node row",
                source,
            })?);
        }
        Ok(out)
    }

    /// All edges in the graph. See [`Self::all_nodes`].
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn all_edges(&self) -> Result<Vec<Edge>> {
        let mut stmt = self
            .conn
            .prepare("SELECT src, dst, kind, confidence FROM edges")
            .map_err(|source| Error::Storage {
                context: "prepare all_edges query",
                source,
            })?;
        let rows = stmt
            .query_map((), |row| {
                Ok(Edge {
                    src: row.get(0)?,
                    dst: row.get(1)?,
                    kind: EdgeKind::from_str(&row.get::<_, String>(2)?),
                    confidence: Confidence::from_str(&row.get::<_, String>(3)?),
                })
            })
            .map_err(|source| Error::Storage {
                context: "run all_edges query",
                source,
            })?;

        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read edge row",
                source,
            })?);
        }
        Ok(out)
    }

    /// All edges paired with their stored evidence provenance.
    ///
    /// # Errors
    /// Returns [`Error::Storage`] if the query fails.
    pub fn all_edges_with_provenance(&self) -> Result<Vec<(Edge, Provenance)>> {
        let mut stmt = self.conn.prepare(
            "SELECT src, dst, kind, confidence, perspective, evidence_kind, evidence_source FROM edges",
        ).map_err(|source| Error::Storage { context: "prepare provenance edge query", source })?;
        let rows = stmt
            .query_map((), |row| {
                Ok((
                    Edge {
                        src: row.get(0)?,
                        dst: row.get(1)?,
                        kind: EdgeKind::from_str(&row.get::<_, String>(2)?),
                        confidence: Confidence::from_str(&row.get::<_, String>(3)?),
                    },
                    Provenance {
                        perspective: Perspective::from_str(&row.get::<_, String>(4)?),
                        evidence_kind: EvidenceKind::from_str(&row.get::<_, String>(5)?),
                        evidence_source: row.get(6)?,
                    },
                ))
            })
            .map_err(|source| Error::Storage {
                context: "run provenance edge query",
                source,
            })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read provenance edge row",
                source,
            })?);
        }
        Ok(out)
    }

    /// Finds a node by exact name. Returns the first match, if any.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] if the query fails.
    pub fn find_by_name(&self, name: &str) -> Result<Option<Node>> {
        self.conn
            .query_row(
                "SELECT id, kind, name, file_path, start_line, end_line, description
                 FROM nodes WHERE name = ?1 LIMIT 1",
                (name,),
                Self::row_to_node,
            )
            .map(Some)
            .or_else(|source| match source {
                rusqlite::Error::QueryReturnedNoRows => Ok(None),
                source => Err(Error::Storage {
                    context: "find node by name",
                    source,
                }),
            })
    }

    fn row_to_node(row: &rusqlite::Row<'_>) -> rusqlite::Result<Node> {
        Ok(Node {
            id: Some(row.get(0)?),
            kind: NodeKind::from_str(&row.get::<_, String>(1)?),
            name: row.get(2)?,
            file_path: row.get(3)?,
            start_line: row.get(4)?,
            end_line: row.get(5)?,
            description: row.get(6)?,
        })
    }

    fn collect_nodes(
        rows: rusqlite::MappedRows<'_, impl FnMut(&rusqlite::Row<'_>) -> rusqlite::Result<Node>>,
    ) -> Result<Vec<Node>> {
        let mut out = Vec::new();
        for row in rows {
            out.push(row.map_err(|source| Error::Storage {
                context: "read node row",
                source,
            })?);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(kind: NodeKind, name: &str) -> Node {
        Node {
            id: None,
            kind,
            name: name.to_string(),
            file_path: "src/lib.rs".to_string(),
            start_line: 1,
            end_line: 10,
            description: None,
        }
    }

    #[test]
    fn inserts_and_finds_node_by_name() {
        let graph = Graph::open_in_memory().unwrap();
        let id = graph.insert_node(&node(NodeKind::Function, "run")).unwrap();

        let found = graph.find_by_name("run").unwrap().unwrap();

        assert_eq!(found.id, Some(id));
        assert_eq!(found.kind, NodeKind::Function);
    }

    #[test]
    fn missing_node_returns_none() {
        let graph = Graph::open_in_memory().unwrap();

        assert_eq!(graph.find_by_name("nope").unwrap(), None);
    }

    #[test]
    fn search_finds_node_by_partial_name_via_fts() {
        let graph = Graph::open_in_memory().unwrap();
        graph
            .insert_node(&node(NodeKind::Function, "bigbang_run"))
            .unwrap();
        graph
            .insert_node(&node(NodeKind::Function, "unrelated"))
            .unwrap();

        let results = graph.search("bigbang*", 10).unwrap();

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "bigbang_run");
    }

    #[test]
    fn callers_returns_edges_pointing_at_dst_with_confidence() {
        let graph = Graph::open_in_memory().unwrap();
        let src_id = graph
            .insert_node(&node(NodeKind::Function, "caller"))
            .unwrap();
        let dst_id = graph
            .insert_node(&node(NodeKind::Function, "callee"))
            .unwrap();
        graph
            .insert_edge(&Edge {
                src: src_id,
                dst: dst_id,
                kind: EdgeKind::Calls,
                confidence: Confidence::Extracted,
            })
            .unwrap();

        let callers = graph.callers(dst_id).unwrap();

        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].0.name, "caller");
        assert_eq!(callers[0].1, EdgeKind::Calls);
        assert_eq!(callers[0].2, Confidence::Extracted);

        let callee_edges = graph.callees(src_id).unwrap();
        assert_eq!(callee_edges.len(), 1);
        assert_eq!(callee_edges[0].0.name, "callee");
    }

    #[test]
    fn explicit_provenance_round_trips() {
        let graph = Graph::open_in_memory().unwrap();
        let provenance = Provenance {
            perspective: Perspective::Declared,
            evidence_kind: EvidenceKind::OpenApi,
            evidence_source: Some("openapi.yaml".into()),
        };
        graph
            .insert_node_with_provenance(&node(NodeKind::Doc, "GET /pets"), &provenance)
            .unwrap();

        let stored = graph.all_nodes_with_provenance().unwrap();
        assert_eq!(stored[0].1, provenance);
    }
}
