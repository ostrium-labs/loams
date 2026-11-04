//! `crdgen` writes the CRDs to `deploy/helm/loams-operator/crds/`, one file
//! per kind. `crdgen --check` writes nothing and exits non-zero when a file
//! is missing, differs, or has no matching kind.

use std::path::PathBuf;
use std::process::ExitCode;

fn crds_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../deploy/helm/loams-operator/crds")
}

fn main() -> ExitCode {
    let check = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--check") => true,
        Some(other) => {
            eprintln!("usage: crdgen [--check] (got {other})");
            return ExitCode::from(2);
        }
    };
    let dir = crds_dir();
    let mut wanted = Vec::new();
    let mut drift = false;
    for crd in loams_operator_api::all_crds() {
        let name = crd.metadata.name.clone().expect("crd name");
        let text = serde_yaml_ng::to_string(&crd).expect("serialise crd");
        let path = dir.join(format!("{name}.yaml"));
        if check {
            if std::fs::read_to_string(&path).ok().as_deref() != Some(text.as_str()) {
                eprintln!("drift: {}", path.display());
                drift = true;
            }
        } else {
            std::fs::create_dir_all(&dir).expect("create crds dir");
            std::fs::write(&path, text).expect("write crd");
            println!("wrote {}", path.display());
        }
        wanted.push(format!("{name}.yaml"));
    }
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let file = entry.file_name().to_string_lossy().into_owned();
            if file.ends_with(".yaml") && !wanted.contains(&file) {
                if check {
                    eprintln!("stale: {}", entry.path().display());
                    drift = true;
                } else {
                    std::fs::remove_file(entry.path()).expect("remove stale crd");
                    println!("removed {}", entry.path().display());
                }
            }
        }
    }
    if drift {
        eprintln!("CRDs drifted; run: cargo run -p loams-operator-api --bin crdgen");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
