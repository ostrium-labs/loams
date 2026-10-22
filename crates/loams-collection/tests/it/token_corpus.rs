//! `token_corpus_matches` (SDKP P0d): the server's own `ConsistencyToken`
//! against `sdks/fixtures/tokens.json`, the corpus every SDK's
//! `<lang>_consistency_token_merge` runs, so the server and the thirteen SDKs
//! agree on how a token parses, prints and merges.

use std::path::Path;

use loams_collection::ConsistencyToken;
use serde_json::Value;

fn corpus() -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../sdks/fixtures/tokens.json");
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_json::from_str(&text).expect("tokens.json is JSON")
}

fn parse(text: &str) -> ConsistencyToken {
    text.parse()
        .unwrap_or_else(|err| panic!("{text:?} should parse: {err}"))
}

#[test]
fn token_corpus_matches() {
    let corpus = corpus();
    for case in corpus["parse"].as_array().expect("parse cases") {
        let text = case["text"].as_str().expect("text");
        let token = parse(text);
        let items: Vec<Value> = token
            .0
            .iter()
            .map(|(stream, partition, offset)| {
                serde_json::json!([stream.0.to_string(), partition, offset.to_string()])
            })
            .collect();
        assert_eq!(Value::Array(items), case["items"], "{text}");
        assert_eq!(token.to_string(), text);
    }
    for case in corpus["merge"].as_array().expect("merge cases") {
        let mut merged = parse(case["a"].as_str().expect("a"));
        merged.merge(&parse(case["b"].as_str().expect("b")));
        assert_eq!(
            merged.to_string(),
            case["expect"].as_str().expect("expect"),
            "{}",
            case["name"]
        );
    }
    for bad in corpus["malformed"].as_array().expect("malformed") {
        let bad = bad.as_str().expect("a string");
        assert!(bad.parse::<ConsistencyToken>().is_err(), "{bad:?} parsed");
    }
    // The corpus is not vacuous.
    assert!(
        corpus["merge"]
            .as_array()
            .is_some_and(|cases| cases.len() >= 5)
    );
}
