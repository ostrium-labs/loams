//! SDKP's `rust_builders_match_corpus`.
//!
//! Every case in `sdks/fixtures/builders/` is driven through the public
//! builders from its neutral spec (the notation is in that directory's
//! README), and the result is compared with `expect` after both are decoded
//! into the named message, so key order and number spelling do not matter.

mod support;

use loams::builders::search::VectorOptions;
use loams::builders::{Fusion, Search, TextRetriever};
use loams::collection_v1::{Query, SearchRequest};
use support::builders::{cases, filter, schema, search};

/// `rust_builders_match_corpus`: every builder case, through the public
/// builders.
#[test]
fn rust_builders_match_corpus() {
    let cases = cases();
    let names: Vec<&str> = cases
        .iter()
        .map(|c| c["name"].as_str().expect("a name"))
        .collect();
    for expected in [
        "schema_notes",
        "filter_term",
        "filter_bool_nested",
        "filter_range_open",
        "filter_ids",
        "search_text_filter_highlight_select",
        "search_vector_ann_params",
        "search_hybrid_rrf",
        "search_hybrid_dbsf",
        "search_hybrid_weighted",
        "search_after_page",
        "search_expand",
    ] {
        assert!(names.contains(&expected), "the corpus has {expected}");
    }
    for case in &cases {
        let name = case["name"].as_str().expect("a name");
        match case["builder"].as_str().expect("a builder") {
            "schema" => {
                assert_eq!(
                    schema(case["spec"].as_array().expect("calls")),
                    case["expect"],
                    "{name}"
                );
            }
            "filter" => {
                let built: Query = filter(&case["spec"]).to_query().expect("decodes");
                let expected: Query =
                    serde_json::from_value(case["expect"].clone()).expect("expect decodes");
                assert_eq!(built, expected, "{name}");
            }
            "search" => {
                let built: SearchRequest = search(&case["spec"]).build().expect("builds");
                let expected: SearchRequest =
                    serde_json::from_value(case["expect"].clone()).expect("expect decodes");
                assert_eq!(built, expected, "{name}");
            }
            other => panic!("no builder {other}"),
        }
    }
}

#[test]
fn a_weighted_fusion_with_the_wrong_number_of_weights_is_refused_before_any_call() {
    let error = Search::on("notes")
        .text(TextRetriever::fields("refund", ["body"]))
        .vector("embedding", &[1.0, 0.0, 0.0, 0.0], VectorOptions::default())
        .fuse(Fusion::weighted([1.0]))
        .build()
        .expect_err("refused");
    assert_eq!(error.code, loams::Code::InvalidArgument);
    assert_eq!(
        error.metadata().get("field").map(String::as_str),
        Some("fusion.weighted_sum.weights")
    );
}
