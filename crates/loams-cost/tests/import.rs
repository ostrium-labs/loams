//! Bill import over the synthetic fixtures in `tests/data` (CA1 Task 3,
//! design §84 §6.2).

use std::io::Write;

use loams_cost::import::{BillFormat, ImportSummary, ImportedLine, Mapped, import_bill};
use loams_cost::{Amount, Basis, CostInputs, ResourceKind, SourceRef};

const INPUTS: &str = r#"
version: 1
currency: USD
mappings:
  - match: { service: "Amazon Simple Storage Service" }
    pool: eu/bucket
    kind: storage
"#;

fn data(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/tests/data/{name}", env!("CARGO_MANIFEST_DIR"))).expect("fixture")
}

fn run(
    inputs: &str,
    bytes: &[u8],
    format: Option<BillFormat>,
) -> (ImportSummary, Vec<ImportedLine>) {
    let inputs = CostInputs::parse(inputs).expect("inputs");
    let mut lines = Vec::new();
    let summary = import_bill(&inputs, "imp1", bytes, format, |l| {
        lines.push(l);
        Ok(())
    })
    .expect("import");
    (summary, lines)
}

fn a(s: &str) -> Amount {
    Amount::parse(s).expect("amount")
}

#[test]
fn focus_detected_and_parsed() {
    let (s, lines) = run(INPUTS, &data("focus.csv"), None);
    assert_eq!(s.format, "focus");
    assert_eq!(s.cost_column, "EffectiveCost");
    assert_eq!(s.lines, 6);
    assert_eq!(s.total, a("127.5"));
    // Compute (40 − 2 + 40), the bucket (31), the tax (8.5) mapped or
    // shared; the NAT gateway (10) is not.
    assert_eq!(s.mapped, a("109"));
    assert_eq!(s.shared, a("8.5"));
    assert_eq!(s.unmapped, a("10"));
    assert_eq!(
        s.unmapped_by_service.get("Amazon Virtual Private Cloud"),
        Some(&a("10"))
    );
    assert_eq!(s.billed_by_invoice.get("inv-1"), Some(&a("135.5")));
    assert_eq!(lines[0].line.cost, a("40"));
    assert_eq!(lines[0].line.billed, a("48"));
    // The bucket's month spreads over 31 days; every source is billed.
    let bucket = &lines[1];
    assert_eq!(bucket.sources.len(), 31);
    assert!(
        bucket
            .sources
            .iter()
            .all(|s| s.basis == Basis::Billed && s.kind == ResourceKind::Storage)
    );
    assert_eq!(
        bucket.sources[0].source,
        SourceRef::BillLine {
            import: "imp1".into(),
            line: 2
        }
    );
    assert_eq!(
        s.first_day.map(|d| d.to_string()).as_deref(),
        Some("2026-10-01")
    );
    assert_eq!(
        s.last_day.map(|d| d.to_string()).as_deref(),
        Some("2026-10-31")
    );
}

#[test]
fn credit_is_negative_source() {
    let (_, lines) = run(INPUTS, &data("focus.csv"), None);
    let credit = &lines[4];
    assert!(matches!(
        credit.mapped,
        Mapped::Pool {
            kind: ResourceKind::Cpu,
            ..
        }
    ));
    assert_eq!(
        credit.sources.iter().map(|s| s.amount).sum::<Amount>(),
        a("-2")
    );
}

#[test]
fn unmapped_lines_reported() {
    let (s, lines) = run(INPUTS, &data("focus.csv"), None);
    let unmapped: Vec<_> = lines
        .iter()
        .filter(|l| l.mapped == Mapped::Unmapped)
        .collect();
    assert_eq!(unmapped.len(), 1);
    assert!(unmapped[0].sources.is_empty());
    assert_eq!(s.unmapped_lines, 1);
}

#[test]
fn import_totals_match_file() {
    for (file, total) in [
        ("focus.csv", "127.5"),
        ("cur2.csv", "72.25"),
        ("cur-legacy.csv", "30"),
    ] {
        let (s, lines) = run(INPUTS, &data(file), None);
        assert_eq!(s.total, a(total), "{file}");
        assert_eq!(s.mapped + s.shared + s.unmapped, s.total, "{file}");
        let sources: Amount = lines
            .iter()
            .flat_map(|l| &l.sources)
            .map(|s| s.amount)
            .sum();
        assert_eq!(sources + s.unmapped, s.total, "{file}");
    }
}

#[test]
fn cur2_detected_and_parsed() {
    let (s, lines) = run(INPUTS, &data("cur2.csv"), None);
    assert_eq!(s.format, "cur2");
    assert_eq!(s.cost_column, "line_item_net_unblended_cost");
    assert_eq!(lines[0].line.cost, a("0.13"));
    assert_eq!(lines[0].line.billed, a("0.145"));
    // `user_loams-pool` is read as `loams-pool` by the built-in rule.
    assert!(matches!(&lines[0].mapped, Mapped::Pool { pool, .. } if pool.as_str() == "eu/compute"));
    // Tax with no resource is shared.
    assert_eq!(lines[2].mapped, Mapped::Shared);
    // Credit: net cost empty, unblended used.
    assert_eq!(lines[3].line.cost, a("-1"));
    assert_eq!(s.unmapped, a("60"));
}

#[test]
fn legacy_cur_names_normalised() {
    let (s, lines) = run(INPUTS, &data("cur-legacy.csv"), None);
    assert_eq!(s.format, "cur_legacy");
    assert!(matches!(
        &lines[1].mapped,
        Mapped::Pool {
            kind: ResourceKind::Io,
            ..
        }
    ));
    assert_eq!(s.unmapped_lines, 0);
}

#[test]
fn generic_csv_column_map() {
    let inputs = "version: 1\ncurrency: USD\nfx:\n  - { from: EUR, to: USD, rate: \"1.08\", day: 2026-10-01, source: \"test rate\" }\n";
    let format = BillFormat::parse(
        "csv",
        Some("start=From,end=To,cost=Amount,currency=@EUR,service=Item,tags=Tags"),
    )
    .expect("format");
    let (s, lines) = run(inputs, &data("generic.csv"), Some(format));
    assert_eq!(s.format, "csv");
    assert_eq!(s.billed_by_currency.get("EUR"), Some(&a("60")));
    // Converted with the declared rate: 30 EUR → 32.4 USD, declared.
    assert_eq!(lines[0].sources[0].amount, a("32.4"));
    assert_eq!(lines[0].sources[0].basis, Basis::Declared);
    assert!(matches!(
        &lines[1].mapped,
        Mapped::Pool {
            kind: ResourceKind::NetworkEgress,
            ..
        }
    ));
    assert_eq!(lines[2].mapped, Mapped::Unmapped);
    assert_eq!(s.total, a("64.8"));
}

#[test]
fn currency_mismatch_without_fx_refused() {
    let format = BillFormat::parse("csv", Some("start=From,end=To,cost=Amount,currency=@EUR"))
        .expect("format");
    let inputs = CostInputs::parse("version: 1\ncurrency: USD\n").expect("inputs");
    let e = import_bill(&inputs, "i", &data("generic.csv")[..], Some(format), |_| {
        Ok(())
    })
    .expect_err("refused");
    assert!(e.to_string().contains("currency_mismatch"), "{e}");
}

#[test]
fn gzip_streamed() {
    let raw = data("focus.csv");
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&raw).expect("gz");
    let gz = gz.finish().expect("gz");
    let (plain, _) = run(INPUTS, &raw, None);
    let (packed, _) = run(INPUTS, &gz, None);
    assert_eq!(plain.total, packed.total);
    assert_eq!(plain.lines, packed.lines);
    // The digest is of the bytes as given.
    assert_ne!(plain.digest, packed.digest);
    assert_eq!(plain.digest.len(), 64);
}

#[test]
fn unknown_header_needs_a_format() {
    let inputs = CostInputs::parse("version: 1\ncurrency: USD\n").expect("inputs");
    assert!(import_bill(&inputs, "i", &data("generic.csv")[..], None, |_| Ok(())).is_err());
}
