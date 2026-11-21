//! BODACC and JOAFE notices: a batch of rows per transaction, each table in one binary COPY.

use std::collections::HashMap;

use chrono::NaiveDate;
use futures::SinkExt;
use lj_gazette::rows::{Row, Rows, Table, Type, Value, ADDRESS, DICT, FILE, OFFICER};
use lj_gazette::Source;
use tokio_postgres::types::ToSql;

use super::DecisionRepository;
use crate::error::{Result, StoreError};

const UNIX_TO_PG_EPOCH_DAYS: i32 = 10_957;

fn epoch() -> NaiveDate {
    NaiveDate::from_ymd_opt(1970, 1, 1).expect("epoch")
}

fn put(buf: &mut Vec<u8>, bytes: &[u8]) {
    buf.extend_from_slice(&(bytes.len() as i32).to_be_bytes());
    buf.extend_from_slice(bytes);
}

fn copy_buffer(table: &Table, rows: &[Row]) -> Result<Vec<u8>> {
    let mut buf = Vec::with_capacity(32 + rows.len() * 256);
    buf.extend_from_slice(b"PGCOPY\n\xff\r\n\0");
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.extend_from_slice(&0i32.to_be_bytes());
    let width = table.columns.len() as i16;
    for row in rows {
        if row.len() != table.columns.len() {
            return Err(StoreError::Invalid(format!(
                "{}: {} values for {} columns",
                table.name,
                row.len(),
                table.columns.len()
            )));
        }
        buf.extend_from_slice(&width.to_be_bytes());
        for (v, (name, ty)) in row.iter().zip(&table.columns) {
            match (v, ty) {
                (Value::Null, _) => buf.extend_from_slice(&(-1i32).to_be_bytes()),
                (Value::Small(x), Type::Small) => put(&mut buf, &x.to_be_bytes()),
                (Value::Int(x), Type::Int) => put(&mut buf, &x.to_be_bytes()),
                (Value::Long(x), Type::Long) => put(&mut buf, &x.to_be_bytes()),
                (Value::Date(d), Type::Date) => {
                    put(&mut buf, &(d - UNIX_TO_PG_EPOCH_DAYS).to_be_bytes())
                }
                (Value::Text(s), Type::Text) => put(&mut buf, s.as_bytes()),
                (Value::Bool(b), Type::Bool) => put(&mut buf, &[u8::from(*b)]),
                (v, ty) => {
                    return Err(StoreError::Invalid(format!(
                        "{}.{name}: {v:?} is not {ty:?}",
                        table.name
                    )))
                }
            }
        }
    }
    buf.extend_from_slice(&(-1i16).to_be_bytes());
    Ok(buf)
}

fn column_list(table: &Table) -> String {
    table
        .columns
        .iter()
        .map(|(n, _)| *n)
        .collect::<Vec<_>>()
        .join(", ")
}

fn read_row(table: &Table, r: &tokio_postgres::Row) -> Row {
    table
        .columns
        .iter()
        .enumerate()
        .map(|(i, (_, ty))| match ty {
            Type::Small => r.get::<_, Option<i16>>(i).map_or(Value::Null, Value::Small),
            Type::Int => r.get::<_, Option<i32>>(i).map_or(Value::Null, Value::Int),
            Type::Long => r.get::<_, Option<i64>>(i).map_or(Value::Null, Value::Long),
            Type::Date => r.get::<_, Option<NaiveDate>>(i).map_or(Value::Null, |d| {
                Value::Date((d - epoch()).num_days() as i32)
            }),
            Type::Text => r
                .get::<_, Option<String>>(i)
                .map_or(Value::Null, Value::Text),
            Type::Bool => r.get::<_, Option<bool>>(i).map_or(Value::Null, Value::Bool),
        })
        .collect()
}

impl DecisionRepository<'_> {
    async fn gazette_copy(&self, table: &Table, rows: &[Row]) -> Result<()> {
        if rows.is_empty() {
            return Ok(());
        }
        let buf = copy_buffer(table, rows)?;
        let sql = format!(
            "COPY {} ({}) FROM STDIN (FORMAT binary)",
            table.name,
            column_list(table)
        );
        let sink = self.conn.copy_in::<_, bytes::Bytes>(&sql).await?;
        futures::pin_mut!(sink);
        sink.send(bytes::Bytes::from(buf)).await?;
        sink.as_mut().finish().await?;
        Ok(())
    }

    /// Held until the session ends: one import at a time hands out dictionary codes and address ids.
    pub async fn gazette_lock(&self) -> Result<bool> {
        Ok(self
            .conn
            .query_one(
                "SELECT pg_try_advisory_lock(hashtext('gazette_import'))",
                &[],
            )
            .await?
            .get(0))
    }

    pub async fn gazette_dicts(&self) -> Result<Vec<(i16, i32, String)>> {
        Ok(self
            .conn
            .query(
                "SELECT kind, code, value FROM gazette_dict ORDER BY kind, code",
                &[],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1), r.get(2)))
            .collect())
    }

    /// The dictionary entries named by `(kind, code)`.
    pub async fn gazette_dict_entries(
        &self,
        codes: &[(i16, i32)],
    ) -> Result<HashMap<(i16, i32), String>> {
        let kinds: Vec<i16> = codes.iter().map(|c| c.0).collect();
        let values: Vec<i32> = codes.iter().map(|c| c.1).collect();
        Ok(self
            .conn
            .query(
                "SELECT d.kind, d.code, d.value FROM gazette_dict d
                 JOIN unnest($1::smallint[], $2::int[]) AS c(kind, code) USING (kind, code)",
                &[&kinds, &values],
            )
            .await?
            .iter()
            .map(|r| ((r.get(0), r.get(1)), r.get(2)))
            .collect())
    }

    pub async fn gazette_last_address(&self) -> Result<i32> {
        Ok(self
            .conn
            .query_one("SELECT coalesce(max(id), 0) FROM gazette_address", &[])
            .await?
            .get(0))
    }

    pub async fn gazette_address_keys(&self, keys: &[i64]) -> Result<Vec<(i64, i32)>> {
        Ok(self
            .conn
            .query(
                "SELECT key, id FROM gazette_address WHERE key = ANY($1)",
                &[&keys],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }

    /// `(checksum, notices)` of each container, by its key.
    pub async fn gazette_files(&self, source: Source) -> Result<HashMap<String, (i64, i32)>> {
        Ok(self
            .conn
            .query(
                "SELECT key, checksum, notices FROM gazette_file WHERE source = $1",
                &[&(source as i16)],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect())
    }

    /// A notice already stored is replaced, its officers with it.
    pub async fn gazette_write(&self, source: Source, rows: &Rows) -> Result<()> {
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<()> = async {
            self.conn
                .batch_execute("SET LOCAL statement_timeout = 0")
                .await?;
            self.gazette_copy(&DICT, &rows.dicts).await?;
            self.gazette_copy(&ADDRESS.table, &rows.addresses).await?;
            let table = &source.layout().table;
            self.conn
                .execute(
                    &format!("DELETE FROM {} WHERE id = ANY($1)", table.name),
                    &[&rows.ids],
                )
                .await?;
            if source == Source::Bodacc {
                self.conn
                    .execute(
                        "DELETE FROM bodacc_officer o USING unnest($1::bigint[]) AS n(id)
                         WHERE o.id >= n.id * 100 AND o.id < n.id * 100 + 100",
                        &[&rows.ids],
                    )
                    .await?;
            }
            self.gazette_copy(table, &rows.notices).await?;
            self.gazette_copy(&OFFICER, &rows.officers).await?;
            let keys: Vec<&str> = rows
                .files
                .iter()
                .filter_map(|f| match &f[1] {
                    Value::Text(k) => Some(k.as_str()),
                    _ => None,
                })
                .collect();
            self.conn
                .execute(
                    "DELETE FROM gazette_file WHERE source = $1 AND key = ANY($2)",
                    &[&(source as i16), &keys],
                )
                .await?;
            self.gazette_copy(&FILE, &rows.files).await?;
            Ok(())
        }
        .await;
        match &result {
            Ok(()) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        result
    }

    pub async fn gazette_rows(&self, table: &Table, ids: &[i64]) -> Result<Vec<Row>> {
        let sql = format!(
            "SELECT {} FROM {} WHERE id = ANY($1)",
            column_list(table),
            table.name
        );
        Ok(self
            .conn
            .query(&sql, &[&ids])
            .await?
            .iter()
            .map(|r| read_row(table, r))
            .collect())
    }

    pub async fn gazette_address_rows(&self, ids: &[i32]) -> Result<Vec<Row>> {
        let table = &ADDRESS.table;
        let sql = format!(
            "SELECT {} FROM {} WHERE id = ANY($1)",
            column_list(table),
            table.name
        );
        Ok(self
            .conn
            .query(&sql, &[&ids])
            .await?
            .iter()
            .map(|r| read_row(table, r))
            .collect())
    }

    /// `(header_shape, header_rest)` of each container, by its key.
    pub async fn gazette_headers(
        &self,
        source: Source,
    ) -> Result<HashMap<String, (Option<i32>, Option<String>)>> {
        Ok(self
            .conn
            .query(
                "SELECT key, header_shape, header_rest FROM gazette_file WHERE source = $1",
                &[&(source as i16)],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), (r.get(1), r.get(2))))
            .collect())
    }
}

/// A BODACC notice as a list shows it.
#[derive(Debug)]
pub struct BodaccNoticeRow {
    pub id: i64,
    pub date: NaiveDate,
    pub famille: i16,
    pub avis: i16,
    pub dept: Option<String>,
    pub tribunal: Option<String>,
    pub name: Option<String>,
    pub first_name: Option<String>,
    pub ville: Option<String>,
}

/// A JOAFE notice as a list shows it.
#[derive(Debug)]
pub struct JoafeNoticeRow {
    pub id: i64,
    pub date: NaiveDate,
    pub action: i16,
    pub type_label: Option<String>,
    pub dept: Option<String>,
    pub titre: Option<String>,
    pub objet: Option<String>,
}

/// What a notice search reads: every word of a text in one field, a SIREN named or taken over, an RNA.
#[derive(Debug, Clone)]
pub enum GazetteQuery {
    All,
    Text(String),
    Siren(i32),
    Rna(String),
}

/// A search over one gazette table. `classes` are `famille` (BODACC) or `action` (JOAFE) codes, `depts` department dictionary codes: `None` leaves the axis free, an empty list matches nothing.
#[derive(Debug, Clone)]
pub struct GazetteFilter {
    pub source: Source,
    pub query: GazetteQuery,
    pub classes: Option<Vec<i16>>,
    pub depts: Option<Vec<i16>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GazetteAxis {
    Class,
    Dept,
}

const BODACC_TEXT: &[&str] = &[
    "name",
    "first_name",
    "usage_name",
    "commercial_name",
    "sigle",
    "enseigne",
    "activity",
    "administration",
    "descriptif",
    "jug_complement",
    "previous_owner",
];
const JOAFE_TEXT: &[&str] = &["titre", "objet", "old_title", "new_title"];

type Params = Vec<Box<dyn ToSql + Sync + Send>>;

impl GazetteFilter {
    fn table(&self) -> &'static str {
        self.source.layout().table.name
    }

    fn class_column(&self) -> &'static str {
        match self.source {
            Source::Bodacc => "famille",
            Source::Joafe => "action",
        }
    }

    fn void(&self, without: Option<GazetteAxis>) -> bool {
        let empty = |axis, v: &Option<Vec<i16>>| {
            without != Some(axis) && v.as_ref().is_some_and(Vec::is_empty)
        };
        empty(GazetteAxis::Class, &self.classes) || empty(GazetteAxis::Dept, &self.depts)
    }

    fn column(&self, axis: GazetteAxis) -> &'static str {
        match axis {
            GazetteAxis::Class => self.class_column(),
            GazetteAxis::Dept => "dept",
        }
    }

    /// The codes each narrowed axis keeps, `without` one of them.
    fn narrowed(&self, without: Option<GazetteAxis>) -> Vec<(&'static str, &Vec<i16>)> {
        [
            (GazetteAxis::Class, &self.classes),
            (GazetteAxis::Dept, &self.depts),
        ]
        .into_iter()
        .filter(|(axis, _)| without != Some(*axis))
        .filter_map(|(axis, codes)| Some((self.column(axis), codes.as_ref()?)))
        .collect()
    }

    /// The axes' predicates over `n`, `without` one of them.
    fn axes(&self, without: Option<GazetteAxis>, params: &mut Params) -> Vec<String> {
        let mut clauses = Vec::new();
        for (column, codes) in self.narrowed(without) {
            params.push(Box::new(codes.clone()));
            clauses.push(format!("n.{column} = ANY(${}::smallint[])", params.len()));
        }
        clauses
    }

    /// Every word of `q` in one text field of the source.
    fn words(&self, q: &str, params: &mut Params) -> String {
        params.push(Box::new(q.to_string()));
        let fields = match self.source {
            Source::Bodacc => BODACC_TEXT,
            Source::Joafe => JOAFE_TEXT,
        };
        let matches: Vec<String> = fields
            .iter()
            .map(|f| {
                format!(
                    "paradedb.match('{f}', ${}, conjunction_mode => true)", // sql-splice-ok: a field of BODACC_TEXT or JOAFE_TEXT
                    params.len()
                )
            })
            .collect();
        format!("paradedb.boolean(should => ARRAY[{}])", matches.join(", "))
    }

    /// The words of `q` and the axes, `without` one, as one index query: an aggregate takes no SQL clause.
    fn indexed(&self, q: &str, without: Option<GazetteAxis>, params: &mut Params) -> String {
        let mut must = vec![self.words(q, params)];
        for (column, codes) in self.narrowed(without) {
            params.push(Box::new(codes.clone()));
            must.push(format!(
                "paradedb.term_set('{column}', ${}::smallint[])", // sql-splice-ok: a column of `column`
                params.len()
            ));
        }
        format!("paradedb.boolean(must => ARRAY[{}])", must.join(", "))
    }

    /// The predicate over the table itself; an empty one when neither a query nor an axis narrows it.
    fn clauses(&self, without: Option<GazetteAxis>, params: &mut Params) -> Vec<String> {
        let mut clauses = self.axes(without, params);
        let query = match &self.query {
            GazetteQuery::All if clauses.is_empty() => None,
            GazetteQuery::All => Some("n.id @@@ paradedb.all()".to_string()),
            GazetteQuery::Text(q) => Some(format!("n.id @@@ {}", self.words(q, params))),
            GazetteQuery::Siren(siren) => {
                params.push(Box::new(*siren));
                Some(format!(
                    "(n.siren = ${0} OR n.previous_siren = ${0})",
                    params.len()
                ))
            }
            GazetteQuery::Rna(rna) => {
                params.push(Box::new(rna.clone()));
                Some(format!("n.rna = ${}", params.len()))
            }
        };
        clauses.extend(query);
        clauses
    }

    /// Ids of a page, newest first: the BM25 index serves the top of a query or a filter, the date index the whole table.
    fn page_ids(&self, limit: i64, offset: i64, params: &mut Params) -> String {
        let clauses = self.clauses(None, params);
        params.push(Box::new(limit));
        params.push(Box::new(offset));
        format!(
            "SELECT id FROM {} n {} ORDER BY n.date DESC, n.id DESC LIMIT ${} OFFSET ${}",
            self.table(),
            where_of(&clauses),
            params.len() - 1,
            params.len()
        )
    }

    /// A `terms` aggregation per axis, keyed by its column.
    fn terms(&self, axes: &[GazetteAxis]) -> serde_json::Value {
        let terms = axes.iter().map(|axis| {
            let column = self.column(*axis);
            let terms = serde_json::json!({"terms": {"field": column, "size": AXIS_SIZE}});
            (column.to_string(), terms)
        });
        serde_json::Value::Object(terms.collect())
    }
}

/// Above the codes of either axis: 11 BODACC families, 6 JOAFE actions, 119 department codes.
const AXIS_SIZE: usize = 1000;

/// The matching notices of a search counted by the codes of each axis.
#[derive(Debug, PartialEq, Eq)]
pub struct GazetteFacets {
    pub classes: Vec<(i16, i64)>,
    pub depts: Vec<(i16, i64)>,
}

/// The buckets of the `terms` aggregation keyed `column`, none when it was not asked.
fn buckets(counted: &serde_json::Value, column: &str) -> Result<Vec<(i16, i64)>> {
    let Some(terms) = counted.get(column) else {
        return Ok(Vec::new());
    };
    if terms["sum_other_doc_count"].as_i64() != Some(0) {
        return Err(StoreError::Invalid(format!(
            "gazette axis {column} truncated"
        )));
    }
    terms["buckets"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|b| match (b["key"].as_i64(), b["doc_count"].as_i64()) {
            (Some(code), Some(count)) => Ok((code as i16, count)),
            _ => Err(StoreError::Invalid(format!(
                "gazette axis {column}: bucket {b}"
            ))),
        })
        .collect()
}

fn where_of(clauses: &[String]) -> String {
    if clauses.is_empty() {
        String::new()
    } else {
        format!("WHERE {}", clauses.join(" AND "))
    }
}

fn refs(params: &Params) -> Vec<&(dyn ToSql + Sync)> {
    params
        .iter()
        .map(|b| b.as_ref() as &(dyn ToSql + Sync))
        .collect()
}

impl DecisionRepository<'_> {
    /// A page of BODACC notices, newest first.
    pub async fn bodacc_search(
        &self,
        f: &GazetteFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<BodaccNoticeRow>> {
        if f.void(None) {
            return Ok(Vec::new());
        }
        let mut params: Params = vec![
            Box::new(lj_gazette::dict::DEPT),
            Box::new(lj_gazette::dict::TRIBUNAL),
        ];
        let ids = f.page_ids(limit, offset, &mut params);
        let sql = format!(
            "SELECT n.id, n.date, n.famille, n.avis, d.value, t.value, n.name, n.first_name,
                    coalesce(a.ville, e.ville)
             FROM ({ids}) h
             JOIN bodacc_notice n ON n.id = h.id
             LEFT JOIN gazette_dict d ON d.kind = $1 AND d.code = n.dept
             LEFT JOIN gazette_dict t ON t.kind = $2 AND t.code = n.tribunal
             LEFT JOIN gazette_address a ON a.id = n.address
             LEFT JOIN gazette_address e ON e.id = n.address_etab
             ORDER BY n.date DESC, n.id DESC"
        );
        Ok(self
            .conn
            .query(&sql, &refs(&params))
            .await?
            .iter()
            .map(|r| BodaccNoticeRow {
                id: r.get(0),
                date: r.get(1),
                famille: r.get(2),
                avis: r.get(3),
                dept: r.get(4),
                tribunal: r.get(5),
                name: r.get(6),
                first_name: r.get(7),
                ville: r.get(8),
            })
            .collect())
    }

    /// A page of JOAFE notices, newest first.
    pub async fn joafe_search(
        &self,
        f: &GazetteFilter,
        limit: i64,
        offset: i64,
    ) -> Result<Vec<JoafeNoticeRow>> {
        if f.void(None) {
            return Ok(Vec::new());
        }
        let mut params: Params = vec![
            Box::new(lj_gazette::dict::JOAFE_TYPE),
            Box::new(lj_gazette::dict::JOAFE_DEPT),
        ];
        let ids = f.page_ids(limit, offset, &mut params);
        let sql = format!(
            "SELECT n.id, n.date, n.action, t.value, d.value, n.titre, n.objet
             FROM ({ids}) h
             JOIN joafe_notice n ON n.id = h.id
             LEFT JOIN gazette_dict t ON t.kind = $1 AND t.code = n.type_label
             LEFT JOIN gazette_dict d ON d.kind = $2 AND d.code = n.dept
             ORDER BY n.date DESC, n.id DESC"
        );
        Ok(self
            .conn
            .query(&sql, &refs(&params))
            .await?
            .iter()
            .map(|r| JoafeNoticeRow {
                id: r.get(0),
                date: r.get(1),
                action: r.get(2),
                type_label: r.get(3),
                dept: r.get(4),
                titre: r.get(5),
                objet: r.get(6),
            })
            .collect())
    }

    /// How many notices match. Without a query, read from the counts by class and department.
    pub async fn gazette_total(&self, f: &GazetteFilter) -> Result<i64> {
        if f.void(None) {
            return Ok(0);
        }
        let mut params = Params::new();
        let sql = match f.query {
            GazetteQuery::All => format!(
                "SELECT coalesce(sum(n.n), 0)::bigint FROM {}_count n {}",
                f.table(),
                where_of(&f.axes(None, &mut params))
            ),
            _ => format!(
                "SELECT count(*) FROM {} n {}",
                f.table(),
                where_of(&f.clauses(None, &mut params))
            ),
        };
        Ok(self.conn.query_one(&sql, &refs(&params)).await?.get(0))
    }

    /// The matching notices counted by class and by department, each axis left free of its own filter.
    pub async fn gazette_facets(&self, f: &GazetteFilter) -> Result<GazetteFacets> {
        let GazetteQuery::Text(q) = &f.query else {
            return Ok(GazetteFacets {
                classes: self.gazette_facet(f, GazetteAxis::Class).await?,
                depts: self.gazette_facet(f, GazetteAxis::Dept).await?,
            });
        };
        let axes = [GazetteAxis::Class, GazetteAxis::Dept];
        let mut params = Params::new();
        // Neither axis narrowed, both count the same notices: one pass.
        let calls = match (&f.classes, &f.depts) {
            (None, None) => vec![(f.indexed(q, None, &mut params), f.terms(&axes).to_string())],
            _ => axes
                .into_iter()
                .filter(|axis| !f.void(Some(*axis)))
                .map(|axis| {
                    (
                        f.indexed(q, Some(axis), &mut params),
                        f.terms(&[axis]).to_string(),
                    )
                })
                .collect(),
        };
        let counted = self.gazette_aggregates(f, calls, params).await?;
        Ok(GazetteFacets {
            classes: buckets(&counted, f.column(GazetteAxis::Class))?,
            depts: buckets(&counted, f.column(GazetteAxis::Dept))?,
        })
    }

    /// The matching notices counted by the codes of one axis, that axis left free, outside the index.
    async fn gazette_facet(&self, f: &GazetteFilter, axis: GazetteAxis) -> Result<Vec<(i16, i64)>> {
        if f.void(Some(axis)) {
            return Ok(Vec::new());
        }
        let column = f.column(axis);
        let mut params = Params::new();
        let sql = match f.query {
            GazetteQuery::All => format!(
                "SELECT n.{column}, sum(n.n)::bigint FROM {}_count n {} GROUP BY 1",
                f.table(),
                where_of(&f.axes(Some(axis), &mut params))
            ),
            _ => format!(
                "SELECT n.{column}, count(*) FROM {} n {} GROUP BY 1",
                f.table(),
                where_of(&f.clauses(Some(axis), &mut params))
            ),
        };
        Ok(self
            .conn
            .query(&sql, &refs(&params))
            .await?
            .iter()
            .filter_map(|r| Some((r.get::<_, Option<i16>>(0)?, r.get(1))))
            .collect())
    }

    /// One `paradedb.aggregate` on the source's BM25 index per `(predicate, aggregation)`, in one statement.
    async fn gazette_aggregates(
        &self,
        f: &GazetteFilter,
        calls: Vec<(String, String)>,
        mut params: Params,
    ) -> Result<serde_json::Value> {
        if calls.is_empty() {
            return Ok(serde_json::json!({}));
        }
        params.push(Box::new(format!("{}_bm25", f.table())));
        let index = params.len();
        let mut columns = Vec::new();
        for (predicate, aggregation) in calls {
            params.push(Box::new(aggregation));
            columns.push(format!(
                "paradedb.aggregate(${index}::text::regclass, {predicate}, ${}::text::json)::text",
                params.len()
            ));
        }
        let row = self
            .conn
            .query_one(&format!("SELECT {}", columns.join(", ")), &refs(&params))
            .await?;
        let mut merged = serde_json::Map::new();
        for i in 0..row.len() {
            match serde_json::from_str(row.get::<_, &str>(i)) {
                Ok(serde_json::Value::Object(answer)) => merged.extend(answer),
                other => {
                    return Err(StoreError::Invalid(format!(
                        "gazette aggregate answered {other:?}"
                    )))
                }
            }
        }
        Ok(serde_json::Value::Object(merged))
    }

    /// `(code, value)` of each department the source writes.
    pub async fn gazette_depts(&self, source: Source) -> Result<Vec<(i16, String)>> {
        let kind = match source {
            Source::Bodacc => lj_gazette::dict::DEPT,
            Source::Joafe => lj_gazette::dict::JOAFE_DEPT,
        };
        Ok(self
            .conn
            .query(
                "SELECT code::smallint, value FROM gazette_dict WHERE kind = $1",
                &[&kind],
            )
            .await?
            .iter()
            .map(|r| (r.get(0), r.get(1)))
            .collect())
    }

    /// Recounts the notices by class and department, which a search without a query reads.
    pub async fn gazette_refresh_counts(&self, source: Source) -> Result<()> {
        self.conn
            .batch_execute(&format!(
                "REFRESH MATERIALIZED VIEW CONCURRENTLY {}_count",
                source.layout().table.name
            ))
            .await?;
        Ok(())
    }

    /// Rewrites the given columns of the notices whose rows differ, keyed by `id`.
    pub async fn gazette_update(&self, table: &Table, rows: &[Row]) -> Result<u64> {
        if rows.is_empty() {
            return Ok(0);
        }
        let columns = column_list(table);
        let set: Vec<String> = table
            .columns
            .iter()
            .skip(1)
            .map(|(c, _)| format!("{c} = u.{c}"))
            .collect();
        self.conn.batch_execute("BEGIN").await?;
        let result: Result<u64> = async {
            self.conn
                .batch_execute(&format!(
                    "CREATE TEMP TABLE gazette_update ON COMMIT DROP AS SELECT {columns} FROM {} WITH NO DATA",
                    table.name
                ))
                .await?;
            self.gazette_copy(
                &Table {
                    name: "gazette_update",
                    columns: table.columns.clone(),
                },
                rows,
            )
            .await?;
            Ok(self
                .conn
                .execute(
                    &format!(
                        "UPDATE {} n SET {} FROM gazette_update u WHERE n.id = u.id",
                        table.name,
                        set.join(", ")
                    ),
                    &[],
                )
                .await?)
        }
        .await;
        match &result {
            Ok(_) => self.conn.batch_execute("COMMIT").await?,
            Err(_) => {
                let _ = self.conn.batch_execute("ROLLBACK").await;
            }
        }
        result
    }
}
