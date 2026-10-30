//! Cost attribution in the `meter` role (design §84 §11–§12; CA1 Task 6).
//!
//! [`Costs`] keeps the cost inputs and bill imports in a [`VersionedDoc`]
//! (`meter-costs`) and answers reports from the House: for each day it runs
//! the driver queries (`loams_meter_house::cost_sql`), turns the rows into
//! usage ([`loams_cost::drivers`]), and runs the same
//! [`loams_cost::pipeline::compute`] the CLI's offline mode runs.
//!
//! Bill lines are kept aggregated per (import, day, pool, kind) with their
//! line count: a bill can have millions of lines, and the document is a
//! control-plane record. The import's file digest identifies the file; the
//! operator keeps it, and `loams cost import --dry-run` re-reads it line by
//! line (ruling R6.1 in plan CA1).
//!
//! Who may do what (§84 §12.2): `meter_admin` writes inputs and imports;
//! reading costs needs `meter_admin` until `cost_viewer` lands with the
//! `CostService` proto (Task 6b). Agents never pass (D1603).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::Arc;

use loams_cost::drivers::{self, DriverRow, Drivers};
use loams_cost::import::{BillFormat, ImportSummary, Mapped, import_bill};
use loams_cost::inputs::{self, ApplyOutcome, CostInputs, StoredVersion};
use loams_cost::pipeline::compute;
use loams_cost::report::{self, InputChange, Report, ReportRequest};
use loams_cost::{
    Amount, Basis, CostError, CostSource, Day, Explanation, Flag, PoolId, ResourceKind, SourceRef,
    explain, trace_id,
};
use loams_meter_house::HouseSql;
use loams_meter_house::cost_sql;
use serde::{Deserialize, Serialize};

use crate::audit::{AuditEvent, AuditSink};
use crate::authz::{Caller, MeterAuthz};
use crate::customers::Clock;
use crate::error::{MeterError, Result};
use crate::store::VersionedDoc;

/// The longest range one report may cover (D2372's recompute bound).
pub const MAX_REPORT_DAYS: i64 = 400;

/// The largest bill one `ImportBill` call takes (gzip as given).
pub const MAX_BILL_BYTES: usize = 512 * 1024 * 1024;

/// One applied `costs.yaml` version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredInputs {
    pub version: u64,
    pub yaml: String,
    pub digest: String,
    /// The first day it applies to (the day it was applied, UTC).
    pub applied_on: Day,
    pub by: String,
    pub reason: String,
}

/// A bill's sources for one (day, pool, kind), summed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AggSource {
    pub day: Day,
    pub pool: PoolId,
    pub kind: ResourceKind,
    pub amount: Amount,
    pub basis: Basis,
    pub lines: u64,
}

/// One bill import.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredImport {
    pub id: String,
    pub filename: String,
    pub inputs_version: u64,
    pub summary: ImportSummary,
    pub sources: Vec<AggSource>,
    pub imported_on: Day,
    pub by: String,
    pub supersedes: Option<String>,
    pub superseded_by: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Snapshot {
    #[serde(default)]
    inputs: Vec<StoredInputs>,
    #[serde(default)]
    imports: Vec<StoredImport>,
}

/// A report with what it was computed from (§84 §9.2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CostReport {
    pub report: Report,
    pub currency: String,
    pub inputs_version: u64,
    pub imports: Vec<String>,
    /// The digest of every input (cost inputs, imports, usage).
    pub digest: String,
    /// The last day the report covers that has closed (to − 1).
    pub as_of: Day,
    /// Σ unmapped cost of the imports in force (not allocated).
    pub unmapped: Amount,
    /// Driver usage no pool claimed, per kind (not allocated).
    pub unpooled: BTreeMap<ResourceKind, Amount>,
    pub unknown_event_types: BTreeSet<String>,
}

/// Cost attribution over the meter's document and the House.
pub struct Costs<H> {
    doc: Arc<dyn VersionedDoc>,
    authz: MeterAuthz,
    house: Arc<H>,
    audit: Arc<dyn AuditSink>,
    clock: Clock,
    writes: Arc<tokio::sync::Mutex<()>>,
}

impl<H> Clone for Costs<H> {
    fn clone(&self) -> Self {
        Costs {
            doc: self.doc.clone(),
            authz: self.authz.clone(),
            house: self.house.clone(),
            audit: self.audit.clone(),
            clock: self.clock.clone(),
            writes: self.writes.clone(),
        }
    }
}

impl<H> fmt::Debug for Costs<H> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Costs").finish_non_exhaustive()
    }
}

fn cost_error(e: CostError) -> MeterError {
    match e {
        CostError::Invalid { field, message } => MeterError::invalid(field, message),
        other => MeterError::invalid("costs", other.to_string()),
    }
}

impl<H: HouseSql> Costs<H> {
    pub fn new(
        doc: Arc<dyn VersionedDoc>,
        authz: MeterAuthz,
        house: Arc<H>,
        audit: Arc<dyn AuditSink>,
        clock: Clock,
    ) -> Self {
        Costs {
            doc,
            authz,
            house,
            audit,
            clock,
            writes: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    fn today(&self) -> Result<Day> {
        Day::of_unix((self.clock)().0.div_euclid(1_000_000_000)).map_err(cost_error)
    }

    async fn load(&self) -> Result<(Option<u64>, Snapshot)> {
        match self.doc.load().await? {
            None => Ok((None, Snapshot::default())),
            Some((v, bytes)) => Ok((
                Some(v),
                serde_json::from_slice(&bytes).map_err(|e| {
                    MeterError::Unavailable(format!("the cost document does not parse: {e}"))
                })?,
            )),
        }
    }

    async fn write<R>(&self, mut f: impl FnMut(&mut Snapshot) -> Result<R>) -> Result<R> {
        let _one = self.writes.lock().await;
        for _ in 0..8 {
            let (v, mut snap) = self.load().await?;
            let out = f(&mut snap)?;
            let bytes = serde_json::to_vec(&snap).map_err(|e| {
                MeterError::Unavailable(format!("the cost document does not serialise: {e}"))
            })?;
            if self.doc.save(v, bytes).await?.is_some() {
                return Ok(out);
            }
        }
        Err(MeterError::Unavailable(
            "the cost document changed concurrently too often; retry".into(),
        ))
    }

    async fn audit(
        &self,
        caller: &Caller,
        action: &str,
        object: String,
        reason: &str,
        details: serde_json::Value,
    ) -> Result<()> {
        self.audit
            .record(AuditEvent {
                action: action.to_string(),
                actor: caller.actor(),
                object,
                org: None,
                reason: reason.to_string(),
                details,
                time: (self.clock)(),
            })
            .await
    }

    /// Reads need `meter_admin` until `cost_viewer` lands (Task 6b).
    async fn require_view(&self, caller: &Caller) -> Result<()> {
        if self.authz.is_meter_admin(caller).await? {
            Ok(())
        } else {
            Err(MeterError::denied(
                "cost_viewer_required",
                "reading costs needs meter_admin (cost_viewer comes with CostService)",
            ))
        }
    }

    /// `ApplyCostInputs` (§84 §6, §12.1): `(version, changed)`.
    pub async fn apply_inputs(
        &self,
        caller: &Caller,
        yaml: &str,
        reason: &str,
    ) -> Result<(u64, bool)> {
        self.authz.require_meter_admin(caller).await?;
        let today = self.today()?;
        let by = caller.actor();
        let outcome = self
            .write(|snap| {
                let latest = snap.inputs.last().map(|s| StoredVersion {
                    version: s.version,
                    digest: s.digest.clone(),
                });
                match inputs::apply(latest.as_ref(), yaml).map_err(|e| match e {
                    CostError::Invalid { message, .. }
                        if message.starts_with("cost_inputs_version_immutable") =>
                    {
                        MeterError::precondition("cost_inputs_version_immutable", message)
                    }
                    other => cost_error(other),
                })? {
                    ApplyOutcome::Unchanged => Ok((latest.map_or(0, |l| l.version), false)),
                    ApplyOutcome::New(parsed) => {
                        snap.inputs.push(StoredInputs {
                            version: parsed.version,
                            yaml: yaml.to_string(),
                            digest: inputs::yaml_digest(yaml),
                            applied_on: today,
                            by: by.clone(),
                            reason: reason.to_string(),
                        });
                        Ok((parsed.version, true))
                    }
                }
            })
            .await?;
        if outcome.1 {
            self.audit(
                caller,
                "meter.cost_inputs.apply",
                format!("cost_inputs:{}", outcome.0),
                reason,
                serde_json::json!({ "digest": inputs::yaml_digest(yaml) }),
            )
            .await?;
        }
        Ok(outcome)
    }

    /// The latest (or the named) inputs version.
    pub async fn inputs(&self, caller: &Caller, version: Option<u64>) -> Result<StoredInputs> {
        self.require_view(caller).await?;
        let (_, snap) = self.load().await?;
        let found = match version {
            Some(v) => snap.inputs.iter().find(|s| s.version == v),
            None => snap.inputs.last(),
        };
        found.cloned().ok_or_else(|| {
            MeterError::not_found(
                "cost_inputs",
                version.map_or("latest".into(), |v| v.to_string()),
            )
        })
    }

    /// `ImportBill` (§84 §6.2): parses, maps and stores the bill's sources,
    /// aggregated. The same file twice is the same import (by digest).
    pub async fn import_bill(
        &self,
        caller: &Caller,
        bytes: &[u8],
        filename: &str,
        format: Option<(&str, Option<&str>)>,
        supersedes: Option<&str>,
    ) -> Result<StoredImport> {
        self.authz.require_meter_admin(caller).await?;
        if bytes.len() > MAX_BILL_BYTES {
            return Err(MeterError::invalid(
                "bill",
                "larger than 512 MiB: split it by month",
            ));
        }
        let (_, snap) = self.load().await?;
        let current = snap.inputs.last().ok_or_else(|| {
            MeterError::precondition(
                "cost_inputs_required",
                "apply costs.yaml before importing a bill",
            )
        })?;
        let parsed = CostInputs::parse(&current.yaml).map_err(cost_error)?;
        let format = format
            .map(|(name, map)| BillFormat::parse(name, map))
            .transpose()
            .map_err(cost_error)?;
        // The id is the file's digest prefix; it is known after the read,
        // so sources are keyed by a placeholder and renamed.
        let mut agg: BTreeMap<(Day, PoolId, ResourceKind, Basis), (Amount, u64)> = BTreeMap::new();
        let summary = import_bill(&parsed, "_", bytes, format, |l| {
            if matches!(l.mapped, Mapped::Unmapped) {
                return Ok(());
            }
            for s in l.sources {
                let slot = agg
                    .entry((s.day, s.pool, s.kind, s.basis))
                    .or_insert((Amount::ZERO, 0));
                slot.0 = slot.0.checked_add(s.amount).ok_or(CostError::Overflow)?;
                slot.1 += 1;
            }
            Ok(())
        })
        .map_err(cost_error)?;
        let id = format!("bill-{}", &summary.digest[..16]);
        let today = self.today()?;
        let by = caller.actor();
        let record = StoredImport {
            id: id.clone(),
            filename: filename.to_string(),
            inputs_version: current.version,
            sources: agg
                .into_iter()
                .map(|((day, pool, kind, basis), (amount, lines))| AggSource {
                    day,
                    pool,
                    kind,
                    amount,
                    basis,
                    lines,
                })
                .collect(),
            summary,
            imported_on: today,
            by,
            supersedes: supersedes.map(str::to_string),
            superseded_by: None,
        };
        let stored = self
            .write(|snap| {
                if let Some(existing) = snap.imports.iter().find(|i| i.id == id) {
                    return Ok((existing.clone(), false));
                }
                if let Some(old) = supersedes {
                    let prev = snap
                        .imports
                        .iter_mut()
                        .find(|i| i.id == old)
                        .ok_or_else(|| MeterError::not_found("bill_import", old))?;
                    if prev.superseded_by.is_some() {
                        return Err(MeterError::precondition(
                            "already_superseded",
                            format!("{old} is already superseded"),
                        ));
                    }
                    prev.superseded_by = Some(id.clone());
                }
                snap.imports.push(record.clone());
                Ok((record.clone(), true))
            })
            .await?;
        if stored.1 {
            self.audit(
                caller,
                "meter.cost_bill.import",
                format!("bill_import:{id}"),
                "",
                serde_json::json!({
                    "digest": stored.0.summary.digest,
                    "lines": stored.0.summary.lines,
                    "total": stored.0.summary.total,
                    "unmapped": stored.0.summary.unmapped,
                    "supersedes": supersedes,
                }),
            )
            .await?;
        }
        Ok(stored.0)
    }

    /// Every import, newest last, without their sources.
    pub async fn imports(&self, caller: &Caller) -> Result<Vec<StoredImport>> {
        self.require_view(caller).await?;
        let (_, snap) = self.load().await?;
        Ok(snap
            .imports
            .into_iter()
            .map(|mut i| {
                i.sources.clear();
                i
            })
            .collect())
    }

    /// The driver rows of one day from the House.
    async fn day_rows(&self, inputs: &CostInputs, day: Day) -> Result<Vec<DriverRow>> {
        let mut rules: Vec<cost_sql::ClassRuleSql> = inputs
            .classes
            .iter()
            .map(|c| cost_sql::ClassRuleSql {
                types: drivers::engine_types(&c.engine),
                class: c.class.clone(),
                when: c.when.clone(),
            })
            .filter(|r| !r.types.is_empty())
            .collect();
        rules.extend(
            drivers::DEFAULT_CLASSES
                .iter()
                .map(|(e, c, w)| cost_sql::ClassRuleSql {
                    types: drivers::engine_types(e),
                    class: c.to_string(),
                    when: w.to_string(),
                }),
        );
        let class = cost_sql::class_expr(&rules, "default")
            .map_err(|e| MeterError::invalid("classes", e))?;
        let start = day.start_unix();
        let counters = cost_sql::counter_query(
            start,
            &drivers::counter_types(),
            &drivers::counter_fields(),
            &class,
        );
        let gauges = cost_sql::gauge_query(start, &drivers::gauges());
        let mut rows = Vec::new();
        for sql in [counters, gauges] {
            let text =
                self.house.query(&sql).await.map_err(|e| {
                    MeterError::Unavailable(format!("cost_unavailable: House: {e}"))
                })?;
            for cols in cost_sql::parse_rows(&text).map_err(MeterError::Unavailable)? {
                rows.push(DriverRow::from_columns(&cols).map_err(cost_error)?);
            }
        }
        Ok(rows)
    }

    /// Everything a report and an explanation need over `[from, to)`.
    async fn computed(
        &self,
        from: Day,
        to: Day,
    ) -> Result<(
        StoredInputs,
        CostInputs,
        Vec<StoredImport>,
        Drivers,
        loams_cost::pipeline::Computed,
        Vec<loams_cost::Line>,
    )> {
        if from >= to {
            return Err(MeterError::invalid("from", "from must be before to"));
        }
        if (to.date() - from.date()).whole_days() > MAX_REPORT_DAYS {
            return Err(MeterError::invalid(
                "to",
                "a report covers at most 400 days",
            ));
        }
        let (_, snap) = self.load().await?;
        let stored = snap.inputs.last().cloned().ok_or_else(|| {
            MeterError::precondition("cost_inputs_required", "no costs.yaml has been applied")
        })?;
        let inputs = CostInputs::parse(&stored.yaml).map_err(cost_error)?;
        let imports: Vec<StoredImport> = snap
            .imports
            .into_iter()
            .filter(|i| i.superseded_by.is_none())
            .collect();
        let bill: Vec<CostSource> = imports
            .iter()
            .flat_map(|i| {
                i.sources.iter().map(|s| CostSource {
                    day: s.day,
                    pool: s.pool.clone(),
                    kind: s.kind,
                    amount: s.amount,
                    basis: s.basis,
                    source: SourceRef::BillImport {
                        import: i.id.clone(),
                        lines: s.lines,
                    },
                    flags: BTreeSet::new(),
                })
            })
            .collect();
        let mut all = Drivers::default();
        let mut day = from;
        while day < to {
            let rows = self.day_rows(&inputs, day).await?;
            let d = drivers::drivers(&inputs, day, &rows).map_err(cost_error)?;
            all.usage.extend(d.usage);
            all.splits.extend(d.splits);
            all.ops.extend(d.ops);
            all.unpooled.extend(d.unpooled);
            all.unknown_types.extend(d.unknown_types);
            day = day
                .next()
                .ok_or_else(|| MeterError::invalid("to", "out of range"))?;
        }
        let digests: Vec<String> = imports.iter().map(|i| i.summary.digest.clone()).collect();
        let c = compute(&inputs, &stored.yaml, &bill, &digests, &all.usage, from, to)
            .map_err(cost_error)?;
        let lines = drivers::apply_splits(&c.allocation.lines, &all.splits).map_err(cost_error)?;
        Ok((stored, inputs, imports, all, c, lines))
    }

    /// `GetCostReport` (§84 §9.2).
    pub async fn report(&self, caller: &Caller, request: &ReportRequest) -> Result<CostReport> {
        self.require_view(caller).await?;
        let (stored, inputs, imports, drivers, c, lines) =
            self.computed(request.from, request.to).await?;
        let (_, snap) = self.load().await?;
        let changes: Vec<InputChange> = snap
            .inputs
            .iter()
            .map(|s| InputChange {
                day: s.applied_on,
                what: format!("costs.yaml v{}", s.version),
            })
            .chain(snap.imports.iter().map(|i| InputChange {
                day: i.imported_on,
                what: format!("bill import {} ({})", i.id, i.filename),
            }))
            .collect();
        let report = report::build(request, &lines, &drivers.ops, &changes).map_err(cost_error)?;
        let mut unpooled: BTreeMap<ResourceKind, Amount> = BTreeMap::new();
        for u in &drivers.unpooled {
            *unpooled.entry(u.kind).or_default() += u.quantity;
        }
        Ok(CostReport {
            report,
            currency: inputs.currency.as_str().to_string(),
            inputs_version: stored.version,
            imports: imports.iter().map(|i| i.id.clone()).collect(),
            digest: c.digest,
            as_of: Day::from_date(
                request
                    .to
                    .date()
                    .previous_day()
                    .unwrap_or(request.to.date()),
            ),
            unmapped: imports.iter().map(|i| i.summary.unmapped).sum(),
            unpooled,
            unknown_event_types: drivers.unknown_types,
        })
    }

    /// `ExplainCost` (§84 §10.2): the line with `trace` in `[from, to)`.
    pub async fn explain(
        &self,
        caller: &Caller,
        trace: &str,
        from: Day,
        to: Day,
    ) -> Result<Explanation> {
        self.require_view(caller).await?;
        let (_, _, _, _, c, lines) = self.computed(from, to).await?;
        let line = lines
            .iter()
            .find(|l| trace_id(l) == trace)
            .ok_or_else(|| MeterError::not_found("cost_trace", trace))?;
        let pd = c
            .allocation
            .pool_days
            .iter()
            .find(|p| p.key == line.key)
            .ok_or_else(|| MeterError::not_found("cost_trace", trace))?;
        let mut e = explain(pd, line);
        if line.flags.contains(&Flag::Spread) {
            e.formula
                .push_str("; idle and shared spread by attributed cost");
        }
        Ok(e)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::MemAudit;
    use crate::store::MemDoc;
    use loams_authz::{Authorizer, BuiltinAuthorizer};
    use loams_meter_core::UtcNanos;
    use loams_meter_house::HouseError;
    use std::sync::Mutex;

    /// A House that answers the driver queries from canned text.
    #[derive(Default)]
    struct FakeHouse {
        counters: String,
        gauges: String,
        queries: Mutex<Vec<String>>,
    }

    impl HouseSql for FakeHouse {
        async fn query(&self, sql: &str) -> std::result::Result<String, HouseError> {
            self.queries.lock().expect("lock").push(sql.to_string());
            Ok(if sql.contains("samples") {
                self.gauges.clone()
            } else {
                self.counters.clone()
            })
        }
    }

    const YAML: &str = r#"
version: 1
currency: USD
assets:
  - id: n1
    pool: fra/compute
    cost: "36.5"
    in_service: 2026-01-01
    life_months: 12
    nodes: [node-1]
    capacity: { cores: 1 }
"#;

    fn costs(house: FakeHouse) -> (Costs<FakeHouse>, Arc<MemAudit>) {
        let authz: Arc<dyn Authorizer> = Arc::new(BuiltinAuthorizer::new());
        let audit = Arc::new(MemAudit::new());
        let clock: Clock = Arc::new(|| UtcNanos(1_793_318_400_000_000_000)); // 2026-10-30
        (
            Costs::new(
                Arc::new(MemDoc::default()),
                MeterAuthz::new(authz, "meter-export"),
                Arc::new(house),
                audit.clone(),
                clock,
            ),
            audit,
        )
    }

    fn agent() -> Caller {
        Caller::Principal(loams_authz::Subject::agent("a1"))
    }

    #[tokio::test]
    async fn apply_inputs_requires_meter_admin() {
        let (c, audit) = costs(FakeHouse::default());
        assert!(c.apply_inputs(&agent(), YAML, "r").await.is_err());
        assert_eq!(
            c.apply_inputs(&Caller::Operator, YAML, "first")
                .await
                .expect("apply"),
            (1, true)
        );
        assert_eq!(
            c.apply_inputs(&Caller::Operator, YAML, "again")
                .await
                .expect("apply"),
            (1, false)
        );
        let changed = YAML.replace("36.5", "40");
        let e = c
            .apply_inputs(&Caller::Operator, &changed, "x")
            .await
            .expect_err("immutable");
        assert_eq!(e.reason(), "cost_inputs_version_immutable");
        assert_eq!(audit.events().len(), 1);
    }

    #[tokio::test]
    async fn agent_cannot_apply_cost_inputs() {
        let (c, _) = costs(FakeHouse::default());
        let e = c
            .apply_inputs(&agent(), YAML, "r")
            .await
            .expect_err("refused");
        assert_eq!(e.reason(), "agent_not_allowed");
    }

    #[tokio::test]
    async fn import_bill_idempotent_by_digest() {
        let (c, _) = costs(FakeHouse::default());
        let bill = b"From,To,Amount,Tags\n2026-10-28,2026-10-29,10,loams-pool=fra/net;loams-kind=network_egress\n2026-10-28,2026-10-29,5,\n";
        let fmt = Some((
            "csv",
            Some("start=From,end=To,cost=Amount,currency=@USD,tags=Tags"),
        ));
        assert!(
            c.import_bill(&Caller::Operator, bill, "b.csv", fmt, None)
                .await
                .is_err()
        );
        c.apply_inputs(&Caller::Operator, YAML, "r")
            .await
            .expect("apply");
        let a = c
            .import_bill(&Caller::Operator, bill, "b.csv", fmt, None)
            .await
            .expect("import");
        assert_eq!(a.summary.unmapped, Amount::from_units(5));
        assert_eq!(a.sources.len(), 1);
        let b = c
            .import_bill(&Caller::Operator, bill, "again.csv", fmt, None)
            .await
            .expect("import");
        assert_eq!(a.id, b.id);
        assert_eq!(c.imports(&Caller::Operator).await.expect("list").len(), 1);
    }

    #[tokio::test]
    async fn supersede_stops_old_lines() {
        let (c, _) = costs(FakeHouse::default());
        c.apply_inputs(&Caller::Operator, YAML, "r")
            .await
            .expect("apply");
        let fmt = Some((
            "csv",
            Some("start=From,end=To,cost=Amount,currency=@USD,tags=Tags"),
        ));
        let v1 = b"From,To,Amount,Tags\n2026-10-28,2026-10-29,10,loams-pool=fra/net;loams-kind=network_egress\n";
        let v2 = b"From,To,Amount,Tags\n2026-10-28,2026-10-29,12,loams-pool=fra/net;loams-kind=network_egress\n";
        let a = c
            .import_bill(&Caller::Operator, v1, "oct.csv", fmt, None)
            .await
            .expect("v1");
        let b = c
            .import_bill(&Caller::Operator, v2, "oct-restated.csv", fmt, Some(&a.id))
            .await
            .expect("v2");
        let req = ReportRequest {
            from: Day::parse("2026-10-28").expect("d"),
            to: Day::parse("2026-10-29").expect("d"),
            group_by: vec![],
            granularity: report::Granularity::Total,
        };
        let r = c.report(&Caller::Operator, &req).await.expect("report");
        assert_eq!(r.imports, vec![b.id.clone()]);
        // 12 (restated bill, unattributed: no usage) + 0.1 (asset day).
        assert_eq!(r.report.totals.input, Amount::parse("12.1").expect("a"));
        assert!(
            c.import_bill(&Caller::Operator, v2, "x", fmt, Some(&a.id))
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn report_reads_drivers_from_the_house() {
        let counters = "type\tsubject\ttenantid\tnode\tengine\tresource\tclass\tevents\tf_cpu_usec\tf_cpu_ms\tf_statements\n\
            io.loams.dev.usage.node.cpu.v1\torg_1\t1/7\tnode-1\thouse\t/h\t_\t10\t43200000000\t0\t0\n\
            io.loams.dev.usage.house.query.v1\torg_1\t1/7\tnode-1\t\t\tscan\t5\t0\t1000\t5000\n";
        let house = FakeHouse {
            counters: counters.to_string(),
            gauges:
                "type\tsubject\ttenantid\tnode\tengine\tresource\tclass\tevents\tbyte_seconds\n"
                    .to_string(),
            ..Default::default()
        };
        let (c, _) = costs(house);
        c.apply_inputs(&Caller::Operator, YAML, "r")
            .await
            .expect("apply");
        let req = ReportRequest {
            from: Day::parse("2026-10-28").expect("d"),
            to: Day::parse("2026-10-29").expect("d"),
            group_by: vec![report::Dimension::Engine, report::Dimension::QueryClass],
            granularity: report::Granularity::Total,
        };
        let r = c.report(&Caller::Operator, &req).await.expect("report");
        // 36.5 over 365 days = 0.1 a day; half the core used → 0.05, idle 0.05.
        let scan = r
            .report
            .rows
            .iter()
            .find(|row| row.group[1].1 == "scan")
            .expect("scan");
        assert_eq!(scan.cost, Amount::parse("0.05").expect("a"));
        assert_eq!(scan.ops, 5000);
        assert_eq!(r.report.totals.idle, Amount::parse("0.05").expect("a"));
        assert_eq!(r.inputs_version, 1);
        // Explain the scan line.
        let e = c
            .explain(&Caller::Operator, &scan.traces[0], req.from, req.to)
            .await
            .expect("explain");
        assert_eq!(e.pool_cost, Amount::parse("0.1").expect("a"));
        assert!(!c.house.queries.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn cost_unavailable_without_house() {
        struct NoHouse;
        impl HouseSql for NoHouse {
            async fn query(&self, _: &str) -> std::result::Result<String, HouseError> {
                Err(HouseError::Transport("none".into()))
            }
        }
        let authz: Arc<dyn Authorizer> = Arc::new(BuiltinAuthorizer::new());
        let c = Costs::new(
            Arc::new(MemDoc::default()),
            MeterAuthz::new(authz, "meter-export"),
            Arc::new(NoHouse),
            Arc::new(MemAudit::new()),
            Arc::new(|| UtcNanos(1_793_318_400_000_000_000)),
        );
        c.apply_inputs(&Caller::Operator, YAML, "r")
            .await
            .expect("apply");
        let req = ReportRequest {
            from: Day::parse("2026-10-28").expect("d"),
            to: Day::parse("2026-10-29").expect("d"),
            group_by: vec![],
            granularity: report::Granularity::Total,
        };
        let e = c
            .report(&Caller::Operator, &req)
            .await
            .expect_err("unavailable");
        assert!(e.to_string().contains("cost_unavailable"), "{e}");
    }
}
