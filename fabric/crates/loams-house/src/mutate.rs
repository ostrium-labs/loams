//! Mutations of lake tables (HE1 Task 9, ruling R9.5): `ALTER TABLE … DELETE`,
//! `UPDATE`, `CLEAR COLUMN` and `DROP PARTITION`, and the lightweight `DELETE
//! FROM` and `UPDATE … SET`.
//!
//! A mutation is a **copy-on-write** of the table at one pinned snapshot, as
//! compaction is ([`crate::compact`]):
//!
//! 1. The statement is read by the House's `ALTER` reader ([`crate::alter`]);
//!    `DELETE FROM t …` and `UPDATE t SET …` are read as the `ALTER TABLE t
//!    DELETE …` and `ALTER TABLE t UPDATE …` they mean.
//! 2. When the statement is one condition (one `DELETE`, `UPDATE` or `DROP
//!    PARTITION`), the rows it touches are counted first, and a mutation that
//!    touches none commits nothing.
//! 3. Every stored row is read in write order ([`crate::views::Mode::Sequenced`]),
//!    the operations are applied in order as nested `SELECT`s (a `DELETE` keeps
//!    the rows whose condition is not true; an `UPDATE` replaces the assigned
//!    columns where it is, cast to the column's type; `CLEAR COLUMN` sets the
//!    column's default; `DROP PARTITION p` deletes the rows whose partition
//!    expression equals `p`), and the result is written in that order through
//!    the write path ([`crate::insert::write_files`]), so rows of one key keep
//!    their order (R9.4).
//! 4. A `replace` snapshot removes every file read and adds the new ones; a
//!    commit that came in between makes it start again, up to
//!    [`MUTATION_ATTEMPTS`] times.
//!
//! The answer comes after the commit, whatever `mutations_sync` says (the House
//! always waits, R9.1). As in ClickHouse: a mutation applies to every row
//! stored, every version of a key included; a condition that is `NULL` keeps
//! the row; a key, partition or version column cannot be updated (`420`); an
//! unknown column is `16`. `DROP PART` and `PARTITION ID '…'` name ClickHouse
//! parts and partition ids, which a lake table does not have (`48`).
//!
//! The cost is the table's size (HE2's deletion vectors, D1552, make it the
//! touched rows').

use crate::alter::{self, AlterOp};
use crate::compact::{self, Compacted};
use crate::ddl::{self, ChColumn, ChTable, Cursor, LakeEngine, quote_name, quote_string};
use crate::errors::{ChError, HouseError};
use crate::expr;
use crate::insert::{InsertContext, Inserted, Source, Target};
use crate::views;
use loams_house_ipc::{Execute, Limits, Output};

/// How many times a mutation starts again when another commit came first.
pub const MUTATION_ATTEMPTS: u32 = 5;

/// A mutation, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MutationPlan {
    /// The table's database.
    pub database: String,
    /// The table.
    pub name: String,
    /// The operations, in order; each [`AlterOp::is_mutation`].
    pub ops: Vec<AlterOp>,
}

impl MutationPlan {
    /// Reads a mutation; `current` is the request's database. `None` when the
    /// statement is not one (an `ALTER` that changes metadata).
    pub fn parse(text: &str, current: &str) -> Result<Option<Self>, HouseError> {
        let text = lightweight_as_alter(text)?.unwrap_or_else(|| text.to_string());
        let alter = alter::parse_alter(&text)?;
        if !alter.ops.iter().any(AlterOp::is_mutation) {
            return Ok(None);
        }
        if !alter.ops.iter().all(AlterOp::is_mutation) {
            return Err(not_implemented(
                "An ALTER on the House either changes rows (DELETE, UPDATE, CLEAR COLUMN, DROP \
                 PARTITION) or changes the table, not both",
            ));
        }
        for (name, _) in &alter.settings {
            if !crate::settings::is_allowed(name) {
                return Err(HouseError::from(ChError::unknown_setting(format!(
                    "Setting {name} is not one an ALTER may set on the House"
                ))));
            }
        }
        let (db, name) = alter.table;
        Ok(Some(Self {
            database: db.unwrap_or_else(|| current.to_string()),
            name,
            ops: alter.ops,
        }))
    }
}

/// Whether a statement's leading words are a mutation: `ALTER TABLE` with one,
/// `DELETE FROM`, or `UPDATE`.
pub fn is_mutation(text: &str) -> bool {
    MutationPlan::parse(text, "default").is_ok_and(|p| p.is_some())
        || lightweight_as_alter(text).is_ok_and(|t| t.is_some())
}

/// `DELETE FROM t [ON CLUSTER c] …` as `ALTER TABLE t [ON CLUSTER c] DELETE …`,
/// and `UPDATE t [ON CLUSTER c] SET …` as `ALTER TABLE t … UPDATE …`; `None` for
/// any other statement.
fn lightweight_as_alter(text: &str) -> Result<Option<String>, HouseError> {
    let mut c = Cursor::new(text);
    let delete = if c.eat_all(&["DELETE", "FROM"]) {
        true
    } else if c.eat("UPDATE") {
        false
    } else {
        return Ok(None);
    };
    let head = c.offset();
    c.qualified()?;
    c.skip_on_cluster()?;
    let table = text[head..c.offset()].trim();
    let rest = if delete {
        c.rest()
    } else {
        c.expect("SET")?;
        c.rest()
    };
    Ok(Some(format!(
        "ALTER TABLE {table} {} {rest}",
        if delete { "DELETE" } else { "UPDATE" }
    )))
}

/// Runs a mutation to its commit; answers like an `INSERT` with no rows and the
/// `replace` snapshot (none when nothing changed).
pub async fn run(ctx: &InsertContext<'_>, plan: &MutationPlan) -> Result<Inserted, HouseError> {
    let started = std::time::Instant::now();
    for _ in 0..MUTATION_ATTEMPTS {
        let snapshot_id = match attempt(ctx, plan).await? {
            Compacted::Abandoned => continue,
            Compacted::Committed(id) => Some(id),
            Compacted::Nothing => None,
        };
        return Ok(Inserted {
            table: format!("{}.{}", plan.database, plan.name),
            written_rows: 0,
            written_bytes: 0,
            snapshot_id,
            elapsed_ns: started.elapsed().as_nanos() as u64,
        });
    }
    Err(HouseError::from(ChError::network_error(format!(
        "The mutation of {}.{}: other commits came first {MUTATION_ATTEMPTS} times; try again",
        plan.database, plan.name
    ))))
}

async fn attempt(ctx: &InsertContext<'_>, plan: &MutationPlan) -> Result<Compacted, HouseError> {
    let lake = ctx.lake;
    let namespace = ctx.user.namespace;
    let (database, name) = (plan.database.as_str(), plan.name.as_str());
    let table = lake
        .lake_table(namespace, database, name, "ALTER TABLE")
        .await?;
    let ch = ChTable::from_lake(database, name, &table);
    let steps = steps(&ch, &plan.ops)?;
    let Some(snapshot) = table.metadata.current_snapshot() else {
        return Ok(Compacted::Nothing);
    };
    let snapshot_id = snapshot.snapshot_id();
    let files: Vec<_> = crate::read::live_files(lake, &table.metadata, snapshot_id)
        .await?
        .into_iter()
        .filter(|f| f.data)
        .collect();
    if files.is_empty() {
        return Ok(Compacted::Nothing);
    }
    let qualified = format!("{}.{}", quote_name(database), quote_name(name));
    if let [step] = steps.as_slice()
        && count(ctx, database, &qualified, step.condition(), snapshot_id).await? == 0
    {
        return Ok(Compacted::Nothing);
    }

    let columns: Vec<&ChColumn> = ch
        .columns
        .iter()
        .filter(|c| c.stored() && c.default_kind != "MATERIALIZED")
        .collect();
    let select = rewrite_select(&ch, &steps, &qualified)?;
    let names: Vec<String> = columns.iter().map(|c| c.name.clone()).collect();
    let list = names
        .iter()
        .map(|c| quote_name(c))
        .collect::<Vec<_>>()
        .join(", ");
    let target = Target {
        database: database.to_string(),
        name: name.to_string(),
        statement: format!("INSERT INTO {qualified} ({list}) {select}"),
        table: table.clone(),
        ch: ch.clone(),
        columns: Some(names),
        settings: Vec::new(),
        source: Source::Select { text: select },
    };
    let pinned = [
        ("loams_snapshot_id".to_string(), snapshot_id.to_string()),
        (crate::read::SEQUENCED.to_string(), "1".to_string()),
    ];
    let pinned_ctx = InsertContext {
        settings: &pinned,
        ..*ctx
    };
    let lease = ctx.pool.acquire(&namespace.to_string()).await?;
    let written = crate::insert::write_files(&pinned_ctx, lease, &target).await?;
    compact::commit_replace(
        ctx,
        database,
        name,
        &table,
        snapshot.manifest_list(),
        &files,
        written,
        ("loams.mutation", mutation_kinds(&plan.ops)),
    )
    .await
}

/// What a snapshot's summary says the mutation was.
fn mutation_kinds(ops: &[AlterOp]) -> &'static str {
    match ops {
        [AlterOp::Delete { .. }] => "delete",
        [AlterOp::Update { .. }] => "update",
        [AlterOp::ClearColumn { .. }] => "clear-column",
        [AlterOp::DropPartition { .. }] => "drop-partition",
        _ => "mutation",
    }
}

/// One operation, checked against the table and made SQL.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    /// Rows whose condition is true are removed.
    Delete { condition: String },
    /// Where the condition is true, the columns take these expressions.
    Update {
        condition: String,
        assignments: Vec<(String, String)>,
    },
}

impl Step {
    fn condition(&self) -> &str {
        match self {
            Self::Delete { condition } | Self::Update { condition, .. } => condition,
        }
    }
}

/// The operations as [`Step`]s: columns checked, partitions made conditions.
fn steps(ch: &ChTable, ops: &[AlterOp]) -> Result<Vec<Step>, HouseError> {
    let mut out = Vec::new();
    for op in ops {
        match op {
            AlterOp::Delete {
                partition,
                predicate,
            } => out.push(Step::Delete {
                condition: within(ch, predicate, partition.as_deref())?,
            }),
            AlterOp::Update {
                assignments,
                partition,
                predicate,
            } => {
                let mut checked = Vec::new();
                for (column, value) in assignments {
                    let column = updatable(ch, column)?;
                    checked.push((column.name.clone(), cast(column, value)?));
                }
                out.push(Step::Update {
                    condition: within(ch, predicate, partition.as_deref())?,
                    assignments: checked,
                });
            }
            AlterOp::ClearColumn {
                if_exists,
                name,
                partition,
            } => {
                if *if_exists && !ch.columns.iter().any(|c| &c.name == name && c.stored()) {
                    continue;
                }
                let column = updatable(ch, name)?;
                let default = if column.default_kind == "DEFAULT" {
                    expr::reprint(&column.default_expression).map_err(|why| {
                        HouseError::from(ChError::access_denied(format!(
                            "Table {}.{}: the default of column {name} is not one the House \
                             runs ({why})",
                            ch.database, ch.name
                        )))
                    })?
                } else {
                    format!("defaultValueOfTypeName({})", quote_string(&column.ch_type))
                };
                out.push(Step::Update {
                    condition: within(ch, "1", partition.as_deref())?,
                    assignments: vec![(column.name.clone(), cast(column, &default)?)],
                });
            }
            AlterOp::DropPartition { partition } => out.push(Step::Delete {
                condition: partition_condition(ch, partition)?,
            }),
            AlterOp::DropPart { part } => {
                return Err(not_implemented(format!(
                    "ALTER TABLE {}.{} DROP PART '{part}': a lake table has no ClickHouse parts; \
                     use DROP PARTITION or DELETE",
                    ch.database, ch.name
                )));
            }
            other => {
                return Err(not_implemented(format!(
                    "{other:?} is not a mutation of a lake table"
                )));
            }
        }
    }
    Ok(out)
}

/// `predicate`, within `IN PARTITION p` when given.
fn within(ch: &ChTable, predicate: &str, partition: Option<&str>) -> Result<String, HouseError> {
    let predicate = predicate.trim();
    if predicate.is_empty() {
        return Err(HouseError::from(ChError::syntax_error(
            "A mutation needs a WHERE condition",
        )));
    }
    Ok(match partition {
        None => format!("({predicate})"),
        Some(p) => format!("(({predicate}) AND {})", partition_condition(ch, p)?),
    })
}

/// `p` of `PARTITION p` as a condition on the table's partition expression.
fn partition_condition(ch: &ChTable, partition: &str) -> Result<String, HouseError> {
    let partition = partition.trim();
    let mut c = Cursor::new(partition);
    if c.eat("ID") {
        return Err(not_implemented(format!(
            "PARTITION ID names a ClickHouse partition id, which table {}.{} does not have; \
             name the partition by its value",
            ch.database, ch.name
        )));
    }
    let key = if ch.partition_by.trim().is_empty() {
        "tuple()".to_string()
    } else {
        expr::reprint(&ch.partition_by).map_err(|why| {
            HouseError::from(ChError::access_denied(format!(
                "Table {}.{}: its partition expression is not one the House runs ({why})",
                ch.database, ch.name
            )))
        })?
    };
    Ok(format!("(({key}) = ({partition}))"))
}

/// The column an `UPDATE` or `CLEAR COLUMN` may change: stored, not computed,
/// and in no key (`420`, as ClickHouse answers); `16` when there is none.
fn updatable<'a>(ch: &'a ChTable, name: &str) -> Result<&'a ChColumn, HouseError> {
    let Some(column) = ch.columns.iter().find(|c| c.name == name) else {
        return Err(ddl::clickhouse_error(
            16,
            "NO_SUCH_COLUMN_IN_TABLE",
            format!(
                "There is no column `{name}` in table {}.{}",
                ch.database, ch.name
            ),
        ));
    };
    let cannot = |why: &str| {
        Err(ddl::clickhouse_error(
            420,
            "CANNOT_UPDATE_COLUMN",
            format!("Cannot UPDATE {why} column `{name}`"),
        ))
    };
    if !column.stored() {
        return cannot("ALIAS");
    }
    if column.default_kind == "MATERIALIZED" {
        return cannot("MATERIALIZED");
    }
    let engine = LakeEngine::parse(&ch.engine).unwrap_or(LakeEngine::MergeTree);
    let version =
        matches!(&engine, LakeEngine::VersionedCollapsing { version, .. } if version == name);
    if version
        || mentions(&ch.order_by, name)
        || mentions(&ch.primary_key, name)
        || mentions(&ch.partition_by, name)
    {
        return cannot("key");
    }
    Ok(column)
}

/// Whether an expression's text names `column`.
fn mentions(text: &str, column: &str) -> bool {
    let c = Cursor::new(text);
    c.toks.iter().any(|t| {
        let raw = c.raw(t);
        (t.word && raw == column)
            || ((raw.starts_with('`') || raw.starts_with('"')) && ddl::unquote(raw) == column)
    })
}

/// `value` cast to `column`'s type (held to the House's safe type form).
fn cast(column: &ChColumn, value: &str) -> Result<String, HouseError> {
    let Some(ty) = views::printable_type(&column.ch_type) else {
        return Err(HouseError::from(ChError::access_denied(format!(
            "The type of column {} is not one the House can name",
            column.name
        ))));
    };
    Ok(format!("CAST(({value}), {})", quote_string(&ty)))
}

/// The `SELECT` that writes the mutated table: every stored row in write order,
/// each step applied in turn.
fn rewrite_select(ch: &ChTable, steps: &[Step], qualified: &str) -> Result<String, HouseError> {
    let stored: Vec<String> = ch
        .columns
        .iter()
        .filter(|c| c.stored())
        .map(|c| quote_name(&c.name))
        .collect();
    let order = views::SEQUENCE_COLUMNS.join(", ");
    let mut sql = format!("SELECT {}, {order} FROM {qualified}", stored.join(", "));
    for step in steps {
        sql = match step {
            Step::Delete { condition } => {
                format!("SELECT * FROM ({sql}) WHERE NOT ifNull({condition}, 0)")
            }
            Step::Update {
                condition,
                assignments,
            } => {
                let select: Vec<String> = ch
                    .columns
                    .iter()
                    .filter(|c| c.stored())
                    .map(|c| {
                        let name = quote_name(&c.name);
                        match assignments.iter().find(|(n, _)| n == &c.name) {
                            Some((_, value)) => {
                                format!("if(ifNull({condition}, 0), {value}, {name}) AS {name}")
                            }
                            None => name,
                        }
                    })
                    .collect();
                format!("SELECT {}, {order} FROM ({sql})", select.join(", "))
            }
        };
    }
    let written: Vec<String> = ch
        .columns
        .iter()
        .filter(|c| c.stored() && c.default_kind != "MATERIALIZED")
        .map(|c| quote_name(&c.name))
        .collect();
    Ok(format!(
        "SELECT {} FROM ({sql}) ORDER BY {order}",
        written.join(", ")
    ))
}

/// `SELECT count() FROM t WHERE condition` at `snapshot_id`, through the read
/// path, as the request's user.
async fn count(
    ctx: &InsertContext<'_>,
    database: &str,
    qualified: &str,
    condition: &str,
    snapshot_id: i64,
) -> Result<u64, HouseError> {
    let sql = format!("SELECT count() FROM {qualified} WHERE ifNull({condition}, 0)");
    let mut lease = ctx.pool.acquire(&ctx.user.namespace.to_string()).await?;
    let result = async {
        let analysis = lease
            .analyze(&sql, ctx.params)
            .await
            .map_err(crate::deny::as_setting_refusal)?;
        crate::gate::admit(&analysis, &sql, crate::classify::Expect::Read, true, None)?;
        let pinned = [("loams_snapshot_id".to_string(), snapshot_id.to_string())];
        let plan = crate::gate::read_plan(
            Some(ctx.lake),
            ctx.user.namespace,
            database,
            ctx.user,
            &mut lease,
            &analysis,
            &sql,
            &pinned,
            None,
            None,
        )
        .await?;
        let out = lease
            .run(Execute {
                query_id: format!("{}-count", ctx.query_id),
                session: None,
                settings: Vec::new(),
                views: plan.views,
                sql: plan.sql,
                format: "TSV".to_string(),
                params: ctx.params.to_vec(),
                limits: Limits::default(),
                input: None,
                output: Output::Blocks,
                database: Some(database.to_string()),
            })
            .await?;
        String::from_utf8_lossy(&out.bytes)
            .trim()
            .parse::<u64>()
            .map_err(|_| {
                HouseError::from(ChError::bad_arguments("count() did not answer a number"))
            })
    }
    .await;
    ctx.pool
        .release(lease, crate::admission::Outcome::Completed);
    result
}

fn not_implemented(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::not_implemented(message.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(engine: &str, order_by: &str, partition_by: &str) -> ChTable {
        let column = |name: &str, ty: &str| ChColumn {
            name: name.to_string(),
            ch_type: ty.to_string(),
            ..ChColumn::default()
        };
        let mut derived = column("m", "UInt64");
        derived.default_kind = "MATERIALIZED".to_string();
        derived.default_expression = "k + 1".to_string();
        ChTable {
            database: "db".to_string(),
            name: "t".to_string(),
            engine: engine.to_string(),
            columns: vec![
                column("k", "UInt64"),
                column("d", "DateTime"),
                column("s", "String"),
                column("v", "UInt64"),
                column("sign", "Int8"),
                derived,
            ],
            order_by: order_by.to_string(),
            partition_by: partition_by.to_string(),
            ..ChTable::default()
        }
    }

    #[test]
    fn lightweight_statements_read_as_alter() {
        let plan = MutationPlan::parse("DELETE FROM db.t ON CLUSTER c WHERE k = 1", "x")
            .expect("parse")
            .expect("a mutation");
        assert_eq!((plan.database.as_str(), plan.name.as_str()), ("db", "t"));
        assert!(
            matches!(&plan.ops[..], [AlterOp::Delete { predicate, .. }] if predicate.trim() == "k = 1")
        );
        let plan = MutationPlan::parse("UPDATE t SET s = 'x', v = v + 1 WHERE k = 1", "cur")
            .expect("parse")
            .expect("a mutation");
        assert_eq!(plan.database, "cur");
        assert!(
            matches!(&plan.ops[..], [AlterOp::Update { assignments, .. }] if assignments.len() == 2)
        );
        assert!(
            MutationPlan::parse("ALTER TABLE t ADD COLUMN x UInt8", "d")
                .expect("parse")
                .is_none()
        );
        assert!(is_mutation("ALTER TABLE t DELETE WHERE 1"));
        assert!(!is_mutation("SELECT 1"));
        let err = MutationPlan::parse("ALTER TABLE t DELETE WHERE 1, ADD COLUMN x UInt8", "d")
            .expect_err("mixed");
        assert!(err.to_string().contains("not both"), "{err}");
    }

    #[test]
    fn updates_are_held_to_clickhouse_rules() {
        let ch = table(
            "VersionedCollapsingMergeTree(sign, v)",
            "(k, toDate(d))",
            "toYYYYMM(d)",
        );
        let code = |e: HouseError| e.to_string();
        assert!(
            code(updatable(&ch, "k").expect_err("key")).contains("Cannot UPDATE key column `k`")
        );
        assert!(code(updatable(&ch, "d").expect_err("key expression")).contains("key"));
        assert!(code(updatable(&ch, "v").expect_err("version")).contains("key"));
        assert!(code(updatable(&ch, "m").expect_err("materialized")).contains("MATERIALIZED"));
        assert!(
            code(updatable(&ch, "nope").expect_err("unknown"))
                .contains("There is no column `nope`")
        );
        assert_eq!(updatable(&ch, "s").expect("s").name, "s");
        assert_eq!(updatable(&ch, "sign").expect("sign").name, "sign");
    }

    #[test]
    fn the_rewrite_applies_each_step_in_write_order() {
        let ch = table("MergeTree", "k", "toYYYYMM(d)");
        let ops = MutationPlan::parse(
            "ALTER TABLE t DELETE WHERE k = 1, UPDATE s = upper(s) IN PARTITION 202610 WHERE v > 2",
            "db",
        )
        .expect("parse")
        .expect("mutation")
        .ops;
        let both = steps(&ch, &ops).expect("steps");
        let sql = rewrite_select(&ch, &both, "db.t").expect("sql");
        assert_eq!(
            sql,
            "SELECT k, d, s, v, sign FROM (SELECT k, d, if(ifNull(((v > 2) AND ((toYYYYMM(`d`)) = \
             (202610))), 0), CAST((upper(s)), 'String'), s) AS s, v, sign, m, __loams_seq, \
             __loams_row FROM (SELECT * FROM (SELECT k, d, s, v, sign, m, __loams_seq, __loams_row \
             FROM db.t) WHERE NOT ifNull((k = 1), 0))) ORDER BY __loams_seq, __loams_row"
        );
        let drop = steps_of(&ch, "ALTER TABLE t DROP PARTITION 202610");
        assert_eq!(
            drop,
            vec![Step::Delete {
                condition: "((toYYYYMM(`d`)) = (202610))".to_string()
            }]
        );
        let err = steps(
            &ch,
            &MutationPlan::parse("ALTER TABLE t DROP PARTITION ID '202610'", "db")
                .expect("parse")
                .expect("mutation")
                .ops,
        )
        .expect_err("partition id");
        assert!(err.to_string().contains("PARTITION ID"), "{err}");
        let clear = steps_of(&ch, "ALTER TABLE t CLEAR COLUMN s");
        assert_eq!(
            clear,
            vec![Step::Update {
                condition: "(1)".to_string(),
                assignments: vec![(
                    "s".to_string(),
                    "CAST((defaultValueOfTypeName('String')), 'String')".to_string()
                )],
            }]
        );
        assert!(steps_of(&ch, "ALTER TABLE t CLEAR COLUMN IF EXISTS nope").is_empty());
    }

    fn steps_of(ch: &ChTable, text: &str) -> Vec<Step> {
        steps(
            ch,
            &MutationPlan::parse(text, "db")
                .expect("parse")
                .expect("mutation")
                .ops,
        )
        .expect("steps")
    }
}
