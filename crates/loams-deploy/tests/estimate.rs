//! The monthly cost estimator (OP2-C Task 2, design 86 D2615).

use loams_cost::Basis;
use loams_deploy::estimate::{
    CostFlag, CostHint, CostItem, StorageKind, estimate_monthly_cost, format_usd,
};
use loams_deploy::{DeployTargets, PriceTable, PriceUnit};
use loams_meter_core::Amount;
use time::macros::date;

fn item(sku: &str, quantity: u64, unit: PriceUnit) -> CostItem {
    CostItem {
        sku: sku.to_string(),
        quantity,
        unit,
        note: None,
    }
}

fn price(sku: &str) -> Amount {
    PriceTable::embedded().get(sku).expect(sku).usd
}

fn aws_k3s_small() -> Vec<CostItem> {
    vec![
        item("aws/ec2/t3.large", 730, PriceUnit::Hour),
        item("aws/ebs/gp3", 60, PriceUnit::GibMonth),
        item("aws/vpc/public-ipv4", 730, PriceUnit::Hour),
        item("aws/kms/key", 1, PriceUnit::Month),
        item("aws/secretsmanager/secret", 4, PriceUnit::Month),
    ]
}

fn target(kind: &str) -> &'static loams_deploy::DeployTarget {
    DeployTargets::embedded().get(kind).expect(kind)
}

#[test]
fn aws_k3s_small_us_east_1() {
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &aws_k3s_small(),
        None,
        PriceTable::embedded(),
    );
    let expected = price("aws/ec2/t3.large").mul_int(730)
        + price("aws/ebs/gp3").mul_int(60)
        + price("aws/vpc/public-ipv4").mul_int(730)
        + price("aws/kms/key").mul_int(1)
        + price("aws/secretsmanager/secret").mul_int(4);
    assert_eq!(estimate.total_usd_month, expected);
    assert_eq!(estimate.lines.len(), 5);
    assert!(estimate.lines.iter().all(|line| line.flags.is_empty()));
    assert!(estimate.complete);
    assert_eq!(estimate.basis, Basis::Estimated);
    assert_eq!(estimate.prices_read, date!(2026 - 11 - 02));
    assert!(!estimate.free_tier_target);
    assert!(!estimate.all_verified);
}

#[test]
fn region_not_in_table_is_flagged() {
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "eu-west-1",
        &aws_k3s_small(),
        None,
        PriceTable::embedded(),
    );
    assert!(estimate.total_usd_month.is_positive());
    assert_eq!(estimate.lines.len(), 5);
    for line in &estimate.lines {
        assert!(
            line.flags.contains(&CostFlag::RegionNotInTable {
                priced_region: "us-east-1".to_string()
            }),
            "{line:?}"
        );
    }
    // The price used is the table's, never zero and never silent.
    let in_table = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &aws_k3s_small(),
        None,
        PriceTable::embedded(),
    );
    assert_eq!(estimate.total_usd_month, in_table.total_usd_month);
    assert!(estimate.complete);
}

#[test]
fn unknown_sku_is_unpriced_not_zeroed() {
    let mut items = aws_k3s_small();
    items.push(item("aws/nope/thing", 3, PriceUnit::Month));
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &items,
        None,
        PriceTable::embedded(),
    );
    assert!(!estimate.complete);
    let line = estimate.lines.last().expect("line");
    assert_eq!(line.sku, "aws/nope/thing");
    assert!(matches!(line.flags.as_slice(), [CostFlag::Unpriced { .. }]));
    assert_eq!(line.usd_month, Amount::ZERO);
    // The priced lines still add up.
    let known = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &aws_k3s_small(),
        None,
        PriceTable::embedded(),
    );
    assert_eq!(estimate.total_usd_month, known.total_usd_month);
}

#[test]
fn unit_mismatch_is_unpriced_not_converted() {
    // The price is per hour; the item says per month.
    let items = [item("aws/ec2/t3.large", 1, PriceUnit::Month)];
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &items,
        None,
        PriceTable::embedded(),
    );
    assert!(!estimate.complete);
    assert_eq!(estimate.total_usd_month, Amount::ZERO);
    assert!(matches!(
        estimate.lines[0].flags.as_slice(),
        [CostFlag::Unpriced { .. }]
    ));
}

#[test]
fn gke_management_fee_is_free_tier() {
    let items = [item("gcp/gke/zonal-management", 730, PriceUnit::Hour)];
    let estimate = estimate_monthly_cost(
        target("gcp-gke"),
        "us-central1",
        &items,
        None,
        PriceTable::embedded(),
    );
    assert_eq!(estimate.total_usd_month, Amount::ZERO);
    assert_eq!(format_usd(estimate.total_usd_month), "0.00");
    assert!(estimate.complete);
    assert!(
        matches!(
            estimate.lines[0].flags.as_slice(),
            [CostFlag::FreeTier { .. }]
        ),
        "{:?}",
        estimate.lines[0]
    );
}

#[test]
fn partial_free_allowance_subtracts_up_to_the_allowance() {
    // 150 GiB out: the first 100 are free.
    let items = [item("aws/network/egress-internet", 150, PriceUnit::Gib)];
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &items,
        None,
        PriceTable::embedded(),
    );
    assert_eq!(
        estimate.total_usd_month,
        price("aws/network/egress-internet").mul_int(50)
    );
    assert!(matches!(
        estimate.lines[0].flags.as_slice(),
        [CostFlag::FreeTier { .. }]
    ));
}

#[test]
fn free_allowance_is_shared_across_lines_of_one_sku() {
    let items = [
        item("aws/network/egress-internet", 80, PriceUnit::Gib),
        item("aws/network/egress-internet", 80, PriceUnit::Gib),
    ];
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &items,
        None,
        PriceTable::embedded(),
    );
    assert_eq!(
        estimate.total_usd_month,
        price("aws/network/egress-internet").mul_int(60)
    );
}

fn hint(storage_gib: u32, egress: u32, managed: Vec<StorageKind>) -> CostHint {
    CostHint {
        cpu_millis: 500,
        memory_mib: 512,
        storage_gib,
        egress_gib_per_month: egress,
        managed,
    }
}

#[test]
fn hint_storage_and_egress_priced() {
    let h = hint(20, 50, vec![]);
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &[],
        Some(&h),
        PriceTable::embedded(),
    );
    assert_eq!(estimate.lines.len(), 2);
    for line in &estimate.lines {
        assert!(line.flags.contains(&CostFlag::FromListingHint), "{line:?}");
    }
    let disk = &estimate.lines[0];
    assert_eq!(disk.sku, "aws/ebs/gp3");
    assert_eq!(disk.quantity, 20);
    assert_eq!(disk.usd_month, price("aws/ebs/gp3").mul_int(20));
    // 50 GiB out is inside the 100 GiB free allowance.
    let egress = &estimate.lines[1];
    assert_eq!(egress.sku, "aws/network/egress-internet");
    assert_eq!(egress.usd_month, Amount::ZERO);
    assert!(
        egress
            .flags
            .iter()
            .any(|flag| matches!(flag, CostFlag::FreeTier { .. }))
    );
    assert_eq!(estimate.total_usd_month, disk.usd_month);
    assert!(estimate.complete);
}

#[test]
fn hint_cpu_and_memory_are_not_priced() {
    let h = hint(0, 0, vec![]);
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &[],
        Some(&h),
        PriceTable::embedded(),
    );
    assert!(estimate.lines.is_empty());
    assert!(estimate.complete);
}

#[test]
fn hint_managed_kafka_unpriced() {
    let h = hint(0, 0, vec![StorageKind::Kafka]);
    let estimate = estimate_monthly_cost(
        target("aws-k3s"),
        "us-east-1",
        &[],
        Some(&h),
        PriceTable::embedded(),
    );
    assert!(!estimate.complete);
    assert_eq!(estimate.lines.len(), 1);
    let line = &estimate.lines[0];
    assert_eq!(line.usd_month, Amount::ZERO);
    assert!(line.flags.contains(&CostFlag::FromListingHint));
    assert!(line.flags.contains(&CostFlag::Unpriced {
        reason: "billed by the managed service (DEP2)".to_string()
    }));
}

#[test]
fn hint_disk_unit_mismatch_is_unpriced() {
    // Azure disks are priced per disk-month, not per GiB-month.
    let h = hint(20, 0, vec![]);
    let estimate = estimate_monthly_cost(
        target("azure-k3s"),
        "eastus",
        &[],
        Some(&h),
        PriceTable::embedded(),
    );
    assert!(!estimate.complete);
    assert!(
        estimate.lines[0]
            .flags
            .iter()
            .any(|flag| matches!(flag, CostFlag::Unpriced { .. }))
    );
    assert_eq!(estimate.total_usd_month, Amount::ZERO);
}

#[test]
fn hint_without_target_sku_is_unpriced() {
    let h = hint(20, 0, vec![]);
    let estimate = estimate_monthly_cost(
        target("aws-free"),
        "us-east-1",
        &[],
        Some(&h),
        PriceTable::embedded(),
    );
    assert!(!estimate.complete);
    assert!(estimate.free_tier_target);
    assert!(matches!(
        estimate.lines[0].flags.as_slice(),
        [CostFlag::FromListingHint, CostFlag::Unpriced { .. }]
    ));
}

#[test]
fn all_verified_needs_every_priced_line_verified() {
    let text = r#"
read = 2026-11-02
[prices."x/a"]
unit = "month"
usd = "1"
regions = ["r"]
source = "list"
url = "u"
verified = true
[prices."x/b"]
unit = "month"
usd = "2"
regions = ["r"]
source = "list"
url = "u"
verified = false
"#;
    let table = PriceTable::parse(text).expect("table");
    let t = target("aws-k3s");
    let both = [
        item("x/a", 1, PriceUnit::Month),
        item("x/b", 1, PriceUnit::Month),
    ];
    let only_a = [item("x/a", 1, PriceUnit::Month)];
    assert!(!estimate_monthly_cost(t, "r", &both, None, &table).all_verified);
    let est = estimate_monthly_cost(t, "r", &only_a, None, &table);
    assert!(est.all_verified);
    assert_eq!(format_usd(est.total_usd_month), "1.00");
    // Nothing priced is not "verified".
    assert!(!estimate_monthly_cost(t, "r", &[], None, &table).all_verified);
}

#[test]
fn format_usd_rounds_half_up() {
    assert_eq!(format_usd(Amount::from_raw(1_234_567_890)), "1.23");
    assert_eq!(format_usd(Amount::from_raw(1_235_000_000)), "1.24");
    assert_eq!(format_usd(Amount::from_raw(1_234_999_999)), "1.23");
    assert_eq!(format_usd(Amount::ZERO), "0.00");
    assert_eq!(format_usd(Amount::from_raw(4_999_999)), "0.00");
    assert_eq!(format_usd(Amount::from_raw(5_000_000)), "0.01");
    assert_eq!(format_usd(Amount::from_units(1234)), "1234.00");
}

#[test]
fn cost_item_deserializes_module_output() {
    let json = r#"[
        {"sku":"aws/ec2/t3.large","quantity":730,"unit":"hour","note":"k3s node"},
        {"sku":"aws/ebs/gp3","quantity":60.0,"unit":"gib-month","note":""},
        {"sku":"aws/kms/key","quantity":1,"unit":"month","note":null},
        {"sku":"aws/network/egress-internet","quantity":5,"unit":"gib"}
    ]"#;
    let items: Vec<CostItem> = serde_json::from_str(json).expect("items");
    assert_eq!(items[0].quantity, 730);
    assert_eq!(items[0].unit, PriceUnit::Hour);
    assert_eq!(items[0].note.as_deref(), Some("k3s node"));
    assert_eq!(items[1].quantity, 60);
    assert_eq!(items[1].unit, PriceUnit::GibMonth);
    assert_eq!(items[1].note, None);
    assert_eq!(items[2].note, None);
    assert_eq!(items[3].unit, PriceUnit::Gib);
    assert_eq!(items[3].note, None);
}

#[test]
fn cost_item_rejects_bad_quantities_and_units() {
    for bad in [
        r#"{"sku":"a","quantity":-1,"unit":"hour","note":""}"#,
        r#"{"sku":"a","quantity":1.5,"unit":"hour","note":""}"#,
        r#"{"sku":"a","quantity":"7","unit":"hour","note":""}"#,
        r#"{"sku":"a","quantity":1,"unit":"fortnight","note":""}"#,
    ] {
        assert!(serde_json::from_str::<CostItem>(bad).is_err(), "{bad}");
    }
}

#[test]
fn cost_hint_round_trips_kebab_case() {
    let json = r#"{"cpu_millis":500,"memory_mib":512,"storage_gib":20,
        "egress_gib_per_month":50,"managed":["sql-postgres","kafka","object","sql-mysql","kv"]}"#;
    let hint: CostHint = serde_json::from_str(json).expect("hint");
    assert_eq!(
        hint.managed,
        vec![
            StorageKind::SqlPostgres,
            StorageKind::Kafka,
            StorageKind::Object,
            StorageKind::SqlMysql,
            StorageKind::Kv
        ]
    );
    let back: CostHint =
        serde_json::from_str(&serde_json::to_string(&hint).expect("ser")).expect("de");
    assert_eq!(back, hint);
}
