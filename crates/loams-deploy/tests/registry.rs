//! The target registry and the dated price table (OP2-C Task 1, design 86).

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use loams_deploy::{DeployTargets, PriceSource, PriceTable, PriceUnit, TargetClass};
use time::macros::date;

const KINDS: [&str; 14] = [
    "aws-k3s",
    "gcp-k3s",
    "azure-k3s",
    "aws-eks",
    "gcp-gke",
    "azure-aks",
    "server-ssh",
    "aws-vm",
    "gcp-vm",
    "azure-vm",
    "aws-free",
    "gcp-free",
    "azure-free",
    "laptop",
];

const OPERATOR_KINDS: [&str; 7] = [
    "aws-k3s",
    "gcp-k3s",
    "azure-k3s",
    "aws-eks",
    "gcp-gke",
    "azure-aks",
    "server-ssh",
];

fn tofu_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../deploy/tofu")
}

#[test]
fn target_kinds_are_exactly() {
    let targets = DeployTargets::embedded();
    let got: BTreeSet<&str> = targets.iter().map(|t| t.kind.as_str()).collect();
    let want: BTreeSet<&str> = KINDS.into_iter().collect();
    assert_eq!(got, want);
    for kind in KINDS {
        assert_eq!(targets.get(kind).map(|t| t.kind.as_str()), Some(kind));
    }
    assert!(targets.get("aws-lambda").is_none());
}

#[test]
fn operator_flag_matches_the_plan() {
    let targets = DeployTargets::embedded();
    for kind in KINDS {
        let t = targets.get(kind).expect("target");
        assert_eq!(
            t.operator,
            OPERATOR_KINDS.contains(&kind),
            "{kind}: operator flag"
        );
    }
}

#[test]
fn every_target_module_exists() {
    let tofu = tofu_dir();
    for t in DeployTargets::embedded().iter().filter(|t| t.available) {
        let main = tofu.join("modules").join(&t.module).join("main.tf");
        assert!(main.is_file(), "{}: missing {}", t.kind, main.display());
        if t.operator {
            let ex = tofu.join("examples").join(&t.kind).join("main.tf");
            assert!(ex.is_file(), "{}: missing {}", t.kind, ex.display());
        }
    }
}

#[test]
fn laptop_is_not_available() {
    let t = DeployTargets::embedded().get("laptop").expect("laptop");
    assert!(!t.available);
    assert!(!t.operator);
    assert_eq!(t.class, TargetClass::Laptop);
}

#[test]
fn vm_and_free_targets_are_not_operator() {
    let targets = DeployTargets::embedded();
    for kind in [
        "aws-vm",
        "gcp-vm",
        "azure-vm",
        "aws-free",
        "gcp-free",
        "azure-free",
    ] {
        let t = targets.get(kind).expect("target");
        assert!(!t.operator, "{kind}");
        assert!(t.available, "{kind}: module already exists");
    }
    assert_eq!(
        targets.get("aws-vm").expect("t").class,
        TargetClass::Compose
    );
    assert_eq!(
        targets.get("azure-free").expect("t").class,
        TargetClass::AppFree
    );
}

#[test]
fn free_targets_are_labelled() {
    let targets = DeployTargets::embedded();
    for t in targets.iter() {
        assert_eq!(
            t.free_tier,
            t.kind.ends_with("-free"),
            "{}: free_tier",
            t.kind
        );
    }
}

#[test]
fn operator_targets_have_sizes_and_priced_skus() {
    let prices = PriceTable::embedded();
    for t in DeployTargets::embedded().iter().filter(|t| t.operator) {
        for size in ["small", "medium"] {
            let s = t
                .sizes
                .get(size)
                .unwrap_or_else(|| panic!("{} {size}", t.kind));
            assert!(s.cpu_millis > 0 && s.memory_mib > 0 && s.nodes > 0);
        }
        assert_eq!(t.reserve.cpu_millis, 1500);
        assert_eq!(t.reserve.memory_mib, 4096);
        if t.kind != "server-ssh" {
            assert!(
                prices.get(&t.disk_sku).is_some(),
                "{}: {}",
                t.kind,
                t.disk_sku
            );
            assert!(
                prices.get(&t.egress_sku).is_some(),
                "{}: {}",
                t.kind,
                t.egress_sku
            );
        }
    }
}

#[test]
fn every_sku_in_modules_is_priced() {
    let re =
        regex::Regex::new(r#""((?:aws|gcp|azure)/[a-z0-9._-]+/[A-Za-z0-9._-]+)""#).expect("regex");
    let prices = PriceTable::embedded();
    let modules = tofu_dir().join("modules");
    let mut seen = 0;
    for entry in fs::read_dir(&modules).expect("modules dir") {
        let dir = entry.expect("entry").path();
        let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if !["-k3s", "-eks", "-gke", "-aks"]
            .iter()
            .any(|s| name.ends_with(s))
        {
            continue;
        }
        for file in fs::read_dir(&dir).expect("module dir") {
            let path = file.expect("file").path();
            if path.extension().and_then(|e| e.to_str()) != Some("tf") {
                continue;
            }
            let text = fs::read_to_string(&path).expect("read");
            for cap in re.captures_iter(&text) {
                seen += 1;
                assert!(
                    prices.get(&cap[1]).is_some(),
                    "{}: SKU {} has no price",
                    path.display(),
                    &cap[1]
                );
            }
        }
    }
    // Not vacuous: an available operator module spells out its disk SKU in full,
    // so the scan above finds and prices the SKUs it emits.
    let mut available_k3s = 0;
    for t in DeployTargets::embedded()
        .iter()
        .filter(|t| t.available && t.operator && t.class == TargetClass::K3s)
    {
        available_k3s += 1;
        let dir = modules.join(&t.module);
        let text: String = ["main.tf", "outputs.tf"]
            .iter()
            .filter_map(|f| fs::read_to_string(dir.join(f)).ok())
            .collect();
        for sku in [&t.disk_sku, "aws/secretsmanager/secret"] {
            if t.cloud == "aws" || sku != "aws/secretsmanager/secret" {
                assert!(
                    text.contains(&format!("\"{sku}\"")),
                    "{}: the module does not emit {sku}",
                    t.kind
                );
            }
        }
    }
    assert!(available_k3s == 0 || seen > 0, "no SKU found in any module");
}

#[test]
fn prices_carry_source_and_date() {
    let prices = PriceTable::embedded();
    assert!(prices.read >= date!(2026 - 11 - 02));
    assert!(!prices.is_empty());
    for (sku, e) in prices.iter() {
        assert!(
            matches!(e.source, PriceSource::List | PriceSource::Estimate),
            "{sku}"
        );
        assert!(e.url.starts_with("https://"), "{sku}: url");
        assert!(!e.regions.is_empty(), "{sku}: regions");
        assert!(!e.usd.is_negative(), "{sku}: usd");
        assert!(!e.verified, "{sku}: nothing is verified against a bill yet");
    }
}

#[test]
fn the_skus_later_tasks_name_are_priced() {
    let prices = PriceTable::embedded();
    for sku in [
        "aws/ec2/t3.large",
        "aws/ec2/t3.xlarge",
        "aws/ebs/gp3",
        "aws/vpc/public-ipv4",
        "aws/kms/key",
        "aws/secretsmanager/secret",
        "aws/eks/control-plane",
        "aws/network/egress-internet",
        "gcp/gce/e2-standard-2",
        "gcp/gce/e2-standard-4",
        "gcp/pd/balanced",
        "gcp/network/external-ipv4",
        "gcp/kms/key-version",
        "gcp/secretmanager/secret-version",
        "gcp/gke/zonal-management",
        "gcp/network/egress-internet",
        "azure/vm/Standard_D2s_v5",
        "azure/vm/Standard_D4s_v5",
        "azure/disk/premium-ssd-p6",
        "azure/disk/premium-ssd-p10",
        "azure/disk/premium-ssd-p3",
        "azure/network/public-ipv4",
        "azure/keyvault/key",
        "azure/aks/control-plane-free",
        "azure/lb/standard",
        "azure/network/egress-internet",
    ] {
        assert!(prices.get(sku).is_some(), "{sku}");
    }
}

#[test]
fn units_and_free_allowances() {
    let prices = PriceTable::embedded();
    assert_eq!(
        prices.get("aws/ec2/t3.large").expect("e").unit,
        PriceUnit::Hour
    );
    assert_eq!(
        prices.get("aws/ebs/gp3").expect("e").unit,
        PriceUnit::GibMonth
    );
    assert_eq!(
        prices.get("aws/network/egress-internet").expect("e").unit,
        PriceUnit::Gib
    );
    assert_eq!(prices.get("aws/kms/key").expect("e").unit, PriceUnit::Month);

    let gke = prices.get("gcp/gke/zonal-management").expect("gke");
    let free = gke.free.as_ref().expect("free allowance");
    assert_eq!(free.quantity, 730);
    assert_eq!(
        free.note,
        "one zonal or Autopilot cluster per billing account"
    );

    let aks = prices.get("azure/aks/control-plane-free").expect("aks");
    assert!(aks.usd.is_zero());
}

#[test]
fn prices_agree_with_the_vm_modules() {
    // The `*-vm` modules carry their own hourly estimates (2026-10-21).
    let prices = PriceTable::embedded();
    let hour = |sku: &str| prices.get(sku).expect("sku").usd;
    let parse = |s: &str| loams_meter_core::Amount::parse(s).expect("amount");
    assert_eq!(hour("aws/ec2/t3.large"), parse("0.0832"));
    assert_eq!(hour("aws/ec2/t3.xlarge"), parse("0.1664"));
    assert_eq!(hour("gcp/gce/e2-standard-2"), parse("0.06701"));
    assert_eq!(hour("gcp/gce/e2-standard-4"), parse("0.13402"));
    assert_eq!(hour("azure/vm/Standard_D2s_v5"), parse("0.096"));
    assert_eq!(hour("azure/vm/Standard_D4s_v5"), parse("0.192"));
}
