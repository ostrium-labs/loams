//! The command line's shape: every subcommand parses its arguments.

use clap::{CommandFactory as _, Parser as _};
use loams_house_mine::cli::{Cli, Command};

#[test]
fn cli_is_consistent() {
    Cli::command().debug_assert();
}

#[test]
fn every_subcommand_parses() {
    for args in [
        &[
            "extract",
            "--app",
            "a",
            "--query-log",
            "q.tsv",
            "--out",
            "a.json",
        ][..],
        &["merge", "a.json", "b.json", "--out", "r.json"],
        &["scrub", "--input", "q.tsv"],
        &["report", "--ranking", "r.json"],
    ] {
        let cli =
            Cli::try_parse_from(std::iter::once("loams-house-mine").chain(args.iter().copied()));
        assert!(cli.is_ok(), "{args:?}: {:?}", cli.err());
    }
    let Ok(Cli {
        command: Command::Merge(merge),
    }) = Cli::try_parse_from(["loams-house-mine", "merge", "a.json", "--out", "r.json"])
    else {
        panic!("merge parses");
    };
    assert_eq!(merge.min_apps, 3);
    assert_eq!(merge.gate, ["langfuse", "openpanel", "signoz"]);
}
