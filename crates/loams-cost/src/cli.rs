//! `loams cost` (design §84 §15, D2377; CA1 Task 8), preview.
//!
//! This release runs **offline**: the inputs, bills and usage events come
//! from files, and the numbers come from the same code the `meter` role
//! runs. The online subcommands (against `CostService`) arrive with that
//! service.
//!
//! ```text
//! loams cost report --inputs costs.yaml [--bill FILE]... [--usage events.ndjson] \
//!     --from 2026-10-01 --to 2026-11-01 [--by engine,query_class] [--granularity total] [--json]
//! loams cost import FILE --inputs costs.yaml [--format focus|cur|cur_legacy|csv --map …] [--json]
//! loams cost export --inputs … --from … --to … --format focus|openmeter [--out FILE]
//! loams cost report --from-bench RESULT.json --price-core-hour 0.05 [--currency USD]
//! ```
//!
//! Every table has a basis column. `--from-bench` output is always
//! `estimated` and flagged `example`: a benchmark's measured CPU time times
//! a price the caller typed in, never a cost anyone paid (D2370).

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Subcommand, ValueEnum};
use loams_meter_core::Amount;

use crate::drivers::{self, Drivers};
use crate::export::{focus, openmeter};
use crate::import::{BillFormat, ImportSummary, import_bill};
use crate::inputs::{CostInputs, Price, PriceSource, PriceUnit};
use crate::pipeline::compute;
use crate::report::{self, Dimension, Granularity, OpsCount, Report, ReportRequest};
use crate::{
    Basis, ConsumerKey, CostError, CostSource, Day, Flag, Line, NONE, PoolId, ResourceKind, Usage,
};

#[derive(Debug, Args)]
pub struct CostArgs {
    #[command(subcommand)]
    pub command: CostCommand,
}

#[derive(Debug, Subcommand)]
pub enum CostCommand {
    /// A cost report: unit costs by engine, tenant, project and query class.
    Report(ReportArgs),
    /// Read a bill (FOCUS, AWS CUR, CSV) and show what maps where; nothing
    /// is stored.
    Import(ImportArgs),
    /// Allocation lines as FOCUS 1.3 CSV or OpenMeter CloudEvents.
    Export(ExportArgs),
}

#[derive(Debug, Args)]
pub struct Sources {
    /// The cost inputs (`costs.yaml`).
    #[arg(long)]
    pub inputs: Option<PathBuf>,
    /// Bill files (plain or gzip); repeatable.
    #[arg(long = "bill")]
    pub bills: Vec<PathBuf>,
    /// The bills' format when it is not detected (`csv` needs `--map`).
    #[arg(long = "bill-format")]
    pub bill_format: Option<String>,
    /// Generic CSV column map: `start=…,end=…,cost=…,currency=@USD,…`.
    #[arg(long = "bill-map")]
    pub bill_map: Option<String>,
    /// Usage events, CloudEvents JSON one per line.
    #[arg(long)]
    pub usage: Option<PathBuf>,
    /// First day (UTC, inclusive).
    #[arg(long)]
    pub from: Option<String>,
    /// Last day (UTC, exclusive).
    #[arg(long)]
    pub to: Option<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum GranularityArg {
    Total,
    Day,
    Week,
    Month,
}

#[derive(Debug, Args)]
pub struct ReportArgs {
    #[command(flatten)]
    pub sources: Sources,
    /// Group by: engine, tenant, project, query_class, pool, kind.
    #[arg(long, default_value = "engine")]
    pub by: String,
    #[arg(long, value_enum, default_value = "total")]
    pub granularity: GranularityArg,
    /// Print the report as JSON.
    #[arg(long)]
    pub json: bool,
    /// An example report from a BQ1 result file (bench/results/bq1/all/*.json).
    #[arg(long = "from-bench")]
    pub from_bench: Option<PathBuf>,
    /// The example price per core-hour for `--from-bench` (required with it).
    #[arg(long = "price-core-hour")]
    pub price_core_hour: Option<String>,
    #[arg(long, default_value = "USD")]
    pub currency: String,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    pub file: PathBuf,
    #[arg(long)]
    pub inputs: PathBuf,
    #[arg(long)]
    pub format: Option<String>,
    #[arg(long)]
    pub map: Option<String>,
    #[arg(long)]
    pub json: bool,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExportFormat {
    Focus,
    Openmeter,
}

#[derive(Debug, Args)]
pub struct ExportArgs {
    #[command(flatten)]
    pub sources: Sources,
    #[arg(long, value_enum)]
    pub format: ExportFormat,
    /// Write here instead of stdout.
    #[arg(long)]
    pub out: Option<PathBuf>,
}

/// Runs `loams cost`.
pub fn run(args: &CostArgs) -> ExitCode {
    let r = match &args.command {
        CostCommand::Report(a) => report_cmd(a),
        CostCommand::Import(a) => import_cmd(a),
        CostCommand::Export(a) => export_cmd(a),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("loams cost: {e}");
            ExitCode::FAILURE
        }
    }
}

fn err(e: impl std::fmt::Display) -> CostError {
    CostError::invalid("cli", e.to_string())
}

fn read(path: &PathBuf) -> Result<Vec<u8>, CostError> {
    std::fs::read(path).map_err(|e| err(format!("{}: {e}", path.display())))
}

/// Everything computed from files, with level-two splits applied.
#[derive(Debug)]
pub struct Offline {
    pub inputs: CostInputs,
    pub lines: Vec<Line>,
    pub ops: Vec<OpsCount>,
    pub digest: String,
    pub imports: Vec<ImportSummary>,
    pub drivers: Drivers,
    pub from: Day,
    pub to: Day,
}

/// Computes `[from, to)` from the files `s` names.
pub fn offline(s: &Sources) -> Result<Offline, CostError> {
    let inputs_path = s
        .inputs
        .as_ref()
        .ok_or_else(|| err("--inputs is required"))?;
    let yaml = String::from_utf8(read(inputs_path)?).map_err(err)?;
    let inputs = CostInputs::parse(&yaml)?;
    let from = Day::parse(s.from.as_deref().ok_or_else(|| err("--from is required"))?)?;
    let to = Day::parse(s.to.as_deref().ok_or_else(|| err("--to is required"))?)?;
    let format = s
        .bill_format
        .as_deref()
        .map(|f| BillFormat::parse(f, s.bill_map.as_deref()))
        .transpose()?;
    let mut bill: Vec<CostSource> = Vec::new();
    let mut imports = Vec::new();
    for path in &s.bills {
        let bytes = read(path)?;
        let name = path
            .file_name()
            .map_or("bill".into(), |n| n.to_string_lossy().to_string());
        let summary = import_bill(&inputs, &name, &bytes[..], format.clone(), |l| {
            bill.extend(l.sources);
            Ok(())
        })?;
        imports.push(summary);
    }
    let mut drivers = Drivers::default();
    if let Some(path) = &s.usage {
        let f = std::fs::File::open(path).map_err(|e| err(format!("{}: {e}", path.display())))?;
        let mut events = Vec::new();
        for (n, line) in BufReader::new(f).lines().enumerate() {
            let line = line.map_err(err)?;
            if line.trim().is_empty() {
                continue;
            }
            events.push(
                serde_json::from_str(&line)
                    .map_err(|e| err(format!("{} line {}: {e}", path.display(), n + 1)))?,
            );
        }
        for (day, rows) in crate::offline::rows_from_events(&inputs, events)? {
            if day < from || day >= to {
                continue;
            }
            let d = drivers::drivers(&inputs, day, &rows)?;
            drivers.usage.extend(d.usage);
            drivers.splits.extend(d.splits);
            drivers.ops.extend(d.ops);
            drivers.unpooled.extend(d.unpooled);
            drivers.unknown_types.extend(d.unknown_types);
        }
    }
    let digests: Vec<String> = imports.iter().map(|i| i.digest.clone()).collect();
    let c = compute(&inputs, &yaml, &bill, &digests, &drivers.usage, from, to)?;
    let lines = drivers::apply_splits(&c.allocation.lines, &drivers.splits)?;
    Ok(Offline {
        inputs,
        lines,
        ops: drivers.ops.clone(),
        digest: c.digest,
        imports,
        drivers,
        from,
        to,
    })
}

/// What [`from_bench`] gives: the inputs it made, the lines, the ops and
/// the day range.
pub type BenchReport = (CostInputs, Vec<Line>, Vec<OpsCount>, Day, Day);

/// An example report from a BQ1 combined-run result: per engine, the
/// measured busy CPU-seconds and ops, priced at `price_core_hour` (an
/// `estimate`). Every line is flagged [`Flag::Example`].
pub fn from_bench(
    json: &serde_json::Value,
    price_core_hour: Amount,
    currency: &str,
) -> Result<BenchReport, CostError> {
    let t0 = json["t0"]
        .as_f64()
        .ok_or_else(|| err("the result has no t0"))?;
    #[allow(clippy::cast_possible_truncation)]
    let day = Day::of_unix(t0 as i64)?;
    let to = day.next().ok_or(CostError::Overflow)?;
    let pool = PoolId::new("bench/laptop")?;
    let mut usage = Vec::new();
    let mut ops = Vec::new();
    for e in json["engines"]
        .as_array()
        .ok_or_else(|| err("the result has no engines"))?
    {
        let engine = crate::drivers::sanitize(e["slot"].as_str().unwrap_or("engine"));
        let busy = e["busy_core_seconds"].to_string();
        let busy =
            Amount::parse(&busy).map_err(|_| err(format!("{engine}: busy_core_seconds {busy}")))?;
        let window = e["window"]["end"].as_f64().unwrap_or(0.0)
            - e["window"]["start"].as_f64().unwrap_or(0.0);
        let qps = e["qps"].as_f64().unwrap_or(0.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let n = (qps * window).round().max(0.0) as u64;
        let consumer = ConsumerKey::new(engine, NONE, NONE, NONE)?;
        usage.push(Usage {
            day,
            pool: pool.clone(),
            kind: ResourceKind::Cpu,
            consumer: consumer.clone(),
            quantity: busy,
            basis: Basis::Measured,
        });
        if n > 0 {
            ops.push(OpsCount {
                day,
                consumer,
                ops: n,
            });
        }
    }
    let currency = loams_meter_core::Currency::parse(currency).map_err(err)?;
    let inputs = CostInputs {
        version: 1,
        currency,
        assets: vec![],
        prices: vec![Price {
            id: "example-core-hour".into(),
            kind: ResourceKind::Cpu,
            scope: pool,
            rate: price_core_hour,
            unit: PriceUnit::CoreHour,
            source: PriceSource::Estimate,
            effective: day,
            until: None,
            note: Some("example input, not a quote".into()),
        }],
        fx: vec![],
        mappings: vec![],
        engines: vec![],
        usage_pools: vec![],
        classes: vec![],
        options: Default::default(),
    };
    inputs.validate()?;
    let c = compute(&inputs, "from-bench", &[], &[], &usage, day, to)?;
    let lines = c
        .allocation
        .lines
        .into_iter()
        .map(|mut l| {
            l.flags.insert(Flag::Example);
            l
        })
        .collect();
    Ok((inputs, lines, ops, day, to))
}

fn dims(by: &str) -> Result<Vec<Dimension>, CostError> {
    by.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(Dimension::parse)
        .collect()
}

fn opt(a: Option<Amount>) -> String {
    a.map_or_else(|| "-".to_string(), |a| a.to_string())
}

fn print_report(r: &Report, currency: &str, header: &[String], example: bool) {
    if example {
        println!(
            "EXAMPLE: a benchmark's measured CPU time times an example price you typed in. Not a cost anyone paid."
        );
    }
    for h in header {
        println!("{h}");
    }
    println!(
        "currency {currency}; input {} = attributed {} + idle {} + unattributed {} + shared {}",
        r.totals.input, r.totals.attributed, r.totals.idle, r.totals.unattributed, r.totals.shared
    );
    let mut head: Vec<String> = vec!["period".into()];
    head.extend(r.request.group_by.iter().map(|d| d.as_str().to_string()));
    head.extend(
        [
            "cost",
            "fully_loaded",
            "ops",
            "per_1m_ops",
            "per_gb_month",
            "per_core_hour",
            "basis",
            "flags",
        ]
        .map(String::from),
    );
    let mut rows: Vec<Vec<String>> = vec![head];
    for row in &r.rows {
        let mut v = vec![row.period.to_string()];
        v.extend(row.group.iter().map(|(_, s)| s.clone()));
        v.push(row.cost.to_string());
        v.push(opt(row.fully_loaded_cost));
        v.push(row.ops.to_string());
        v.push(opt(row.unit.per_million_ops));
        v.push(opt(row.unit.per_gb_month));
        v.push(opt(row.unit.per_core_hour));
        v.push(row.basis.to_string());
        v.push(
            row.flags
                .iter()
                .map(|f| f.as_str())
                .collect::<Vec<_>>()
                .join(","),
        );
        rows.push(v);
    }
    let widths: Vec<usize> = (0..rows[0].len())
        .map(|i| {
            rows.iter()
                .map(|r| r.get(i).map_or(0, String::len))
                .max()
                .unwrap_or(0)
        })
        .collect();
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .zip(&widths)
            .map(|(c, w)| format!("{c:<w$}"))
            .collect();
        println!("{}", line.join("  ").trim_end());
    }
}

fn report_cmd(a: &ReportArgs) -> Result<(), CostError> {
    let granularity = match a.granularity {
        GranularityArg::Total => Granularity::Total,
        GranularityArg::Day => Granularity::Day,
        GranularityArg::Week => Granularity::Week,
        GranularityArg::Month => Granularity::Month,
    };
    if let Some(path) = &a.from_bench {
        let price = a
            .price_core_hour
            .as_deref()
            .ok_or_else(|| err("--from-bench needs --price-core-hour (Loams supplies no price)"))?;
        let price = Amount::parse(price).map_err(err)?;
        let json: serde_json::Value = serde_json::from_slice(&read(path)?).map_err(err)?;
        let (inputs, lines, ops, from, to) = from_bench(&json, price, &a.currency)?;
        let req = ReportRequest {
            from,
            to,
            group_by: dims(&a.by)?,
            granularity,
        };
        let r = report::build(&req, &lines, &ops, &[])?;
        if a.json {
            let out = serde_json::json!({ "example": true, "currency": inputs.currency.as_str(), "report": r });
            println!("{}", serde_json::to_string_pretty(&out).map_err(err)?);
        } else {
            let host = json["host"].to_string();
            print_report(
                &r,
                inputs.currency.as_str(),
                &[format!(
                    "source: {} (host {host}); price {price} per core-hour (example input)",
                    path.display()
                )],
                true,
            );
        }
        return Ok(());
    }
    let o = offline(&a.sources)?;
    let req = ReportRequest {
        from: o.from,
        to: o.to,
        group_by: dims(&a.by)?,
        granularity,
    };
    let r = report::build(&req, &o.lines, &o.ops, &[])?;
    let unmapped: Amount = o.imports.iter().map(|i| i.unmapped).sum();
    if a.json {
        let out = serde_json::json!({
            "currency": o.inputs.currency.as_str(),
            "inputs_version": o.inputs.version,
            "digest": o.digest,
            "unmapped": unmapped,
            "unpooled": o.drivers.unpooled,
            "unknown_event_types": o.drivers.unknown_types,
            "report": r,
        });
        println!("{}", serde_json::to_string_pretty(&out).map_err(err)?);
    } else {
        let mut header = vec![format!(
            "costs.yaml v{}; digest {}",
            o.inputs.version, o.digest
        )];
        if !unmapped.is_zero() {
            header.push(format!("unmapped bill cost (not allocated): {unmapped}"));
        }
        if !o.drivers.unpooled.is_empty() {
            header.push(format!(
                "usage rows no pool claims (not allocated): {}",
                o.drivers.unpooled.len()
            ));
        }
        if !o.drivers.unknown_types.is_empty() {
            let t: BTreeSet<&String> = o.drivers.unknown_types.iter().collect();
            header.push(format!("event types not in the driver catalogue: {t:?}"));
        }
        print_report(&r, o.inputs.currency.as_str(), &header, false);
    }
    Ok(())
}

fn import_cmd(a: &ImportArgs) -> Result<(), CostError> {
    let yaml = String::from_utf8(read(&a.inputs)?).map_err(err)?;
    let inputs = CostInputs::parse(&yaml)?;
    let format = a
        .format
        .as_deref()
        .map(|f| BillFormat::parse(f, a.map.as_deref()))
        .transpose()?;
    let bytes = read(&a.file)?;
    let s = import_bill(&inputs, "dry-run", &bytes[..], format, |_| Ok(()))?;
    if a.json {
        println!("{}", serde_json::to_string_pretty(&s).map_err(err)?);
        return Ok(());
    }
    println!(
        "{} ({} format, cost from {}), digest {}",
        a.file.display(),
        s.format,
        s.cost_column,
        s.digest
    );
    println!(
        "lines {}: mapped {} ({}), shared {} ({}), unmapped {} ({})",
        s.lines, s.mapped_lines, s.mapped, s.shared_lines, s.shared, s.unmapped_lines, s.unmapped
    );
    println!("total {} {}", s.total, inputs.currency.as_str());
    for (service, amount) in &s.unmapped_by_service {
        println!(
            "  unmapped: {} {amount}",
            if service.is_empty() {
                "(no service)"
            } else {
                service
            }
        );
    }
    for (inv, billed) in &s.billed_by_invoice {
        println!("  invoice {inv}: billed {billed}");
    }
    Ok(())
}

fn export_cmd(a: &ExportArgs) -> Result<(), CostError> {
    let o = offline(&a.sources)?;
    let mut out: Box<dyn Write> = match &a.out {
        Some(p) => {
            Box::new(std::fs::File::create(p).map_err(|e| err(format!("{}: {e}", p.display())))?)
        }
        None => Box::new(std::io::stdout()),
    };
    match a.format {
        ExportFormat::Focus => {
            let n = focus::write_focus(
                &mut out,
                &o.lines,
                &focus::FocusOptions::new(o.inputs.currency.as_str()),
            )?;
            eprintln!("{n} FOCUS rows");
        }
        ExportFormat::Openmeter => {
            let events = openmeter::cost_events(&o.lines, o.inputs.currency.as_str(), &o.digest)?;
            let batch = openmeter::cloudevents(
                &events,
                "loams://offline/cost",
                openmeter::ALLOCATION_TYPE,
                0,
            );
            writeln!(out, "{}", serde_json::to_string(&batch).map_err(err)?).map_err(err)?;
            eprintln!("{} cost events", events.len());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_report_is_example_estimated() {
        let json = serde_json::json!({
            "t0": 1793257906.0,
            "host": {"cpu": "laptop"},
            "engines": [
                {"slot": "meter", "busy_core_seconds": 29.6, "qps": 392602, "window": {"start": 0.0, "end": 60.0}},
                {"slot": "pg", "busy_core_seconds": 117.5, "qps": 93451, "window": {"start": 0.0, "end": 60.0}},
            ]
        });
        let (_, lines, ops, from, to) =
            from_bench(&json, Amount::parse("0.05").expect("a"), "USD").expect("bench");
        assert!(
            lines
                .iter()
                .all(|l| l.basis == Basis::Estimated && l.flags.contains(&Flag::Example))
        );
        let req = ReportRequest {
            from,
            to,
            group_by: vec![Dimension::Engine],
            granularity: Granularity::Total,
        };
        let r = report::build(&req, &lines, &ops, &[]).expect("report");
        let pg = r
            .rows
            .iter()
            .find(|row| row.group[0].1 == "pg")
            .expect("pg");
        // The pool is priced as a whole (147.1 core-s at 0.05 per core-hour
        // = 0.002043056) and split by usage: pg's 117.5 of 147.1.
        assert_eq!(pg.cost.to_string(), "0.001631945");
        assert_eq!(pg.ops, 5_607_060);
        assert!(pg.unit.per_million_ops.is_some());
        assert!(pg.flags.contains(&Flag::Example));
    }

    #[test]
    fn offline_report_from_files() {
        let base = format!("{}/tests/data", env!("CARGO_MANIFEST_DIR"));
        let s = Sources {
            inputs: Some(PathBuf::from(format!("{base}/costs.yaml"))),
            bills: vec![PathBuf::from(format!("{base}/focus.csv"))],
            bill_format: None,
            bill_map: None,
            usage: Some(PathBuf::from(format!("{base}/usage.ndjson"))),
            from: Some("2026-10-01".into()),
            to: Some("2026-10-02".into()),
        };
        let o = offline(&s).expect("offline");
        let req = ReportRequest {
            from: o.from,
            to: o.to,
            group_by: vec![Dimension::Engine, Dimension::QueryClass],
            granularity: Granularity::Total,
        };
        let r = report::build(&req, &o.lines, &o.ops, &[]).expect("report");
        assert!(r.rows.iter().any(|row| row.group[1].1 == "scan"));
        let inputs: Amount = o.lines.iter().map(|l| l.amount).sum();
        assert_eq!(r.totals.input, inputs);
        assert_eq!(o.imports.len(), 1);
    }
}
