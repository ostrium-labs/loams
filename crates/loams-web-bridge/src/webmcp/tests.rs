//! The WebMCP tools' own tests (AP1d Task 3).
//!
//! `cargo test -p loams-web-bridge webmcp` runs these and the two conformance
//! tests in `tests/it/webmcp.rs`, which run the same contract against both
//! providers over the deterministic drivers.

use super::*;
use serde_json::json;

fn tool_json(name: &str) -> Value {
    json!({
        "name": name,
        "title": format!("{name} title"),
        "description": format!("what {name} does"),
        "inputSchema": {"type": "object", "required": ["query"]},
        "annotations": {"readOnlyHint": true}
    })
}

#[test]
fn a_name_from_the_draft_is_accepted() {
    for name in [
        "a",
        "add_to_cart",
        "a.b-c",
        "Tool9",
        &"x".repeat(MAX_TOOL_NAME_CHARS),
    ] {
        let parsed = WebmcpToolName::parse(name);
        assert!(parsed.is_ok(), "{name} should parse: {parsed:?}");
    }
}

#[test]
fn an_invalid_name_is_refused_before_anything_is_sent() {
    for name in ["", "add to cart", "add/to/cart", "tool\n", "tool;DROP", "é"] {
        let error = WebmcpToolName::parse(name)
            .err()
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(
            error.to_string().contains("list_webmcp_tools"),
            "{name}: {error}"
        );
    }
    let long = "x".repeat(MAX_TOOL_NAME_CHARS + 1);
    assert!(WebmcpToolName::parse(&long).is_err());
}

#[test]
fn a_listing_renders_one_line_per_tool() {
    let raw = envelope_tools(&[tool_json("add_to_cart"), tool_json("find_items")]);
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(listing.availability.is_available());
    assert_eq!(listing.tools.len(), 2);
    let rendered = listing.render();
    assert!(rendered.contains("add_to_cart"), "{rendered}");
    assert!(rendered.contains("[readOnly]"), "{rendered}");
    assert!(rendered.contains("requires=query"), "{rendered}");
    assert_eq!(listing.tools[0].required_inputs(), ["query"]);
}

#[test]
fn an_absent_model_context_is_a_clear_answer_not_an_error() {
    let raw = envelope_absent("not-exposed");
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!listing.availability.is_available());
    assert_eq!(
        listing.availability.absent(),
        Some(&WebmcpAbsent::NotExposed)
    );
    let rendered = listing.render();
    assert!(rendered.contains("no WebMCP tools"), "{rendered}");
    assert!(rendered.contains("take_snapshot"), "{rendered}");
}

#[test]
fn the_permissions_policy_refusal_is_named_for_what_it_is() {
    let raw = envelope_absent("NotAllowedError");
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        listing.availability.absent(),
        Some(&WebmcpAbsent::BlockedByPermissionsPolicy)
    );
    assert!(listing.render().contains("Permissions Policy"));
}

#[test]
fn a_filter_narrows_the_listing_in_the_page_and_here() {
    let raw = envelope_tools(&[tool_json("add_to_cart"), tool_json("find_items")]);
    let request = ListWebmcpToolsRequest::new().with_filter("FIND");
    let listing = parse_listing(&raw, &request).unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(listing.tools.len(), 1);
    assert_eq!(listing.tools[0].name.as_str(), "find_items");
    assert!(listing.render().contains("1 of 2"), "{}", listing.render());
}

#[test]
fn an_empty_filter_means_everything() {
    assert!(matches_filter(Some(""), "anything"));
    assert!(matches_filter(None, "anything"));
    assert!(!matches_filter(Some("zzz"), "anything"));
}

#[test]
fn an_entry_that_names_no_usable_tool_is_counted_not_rendered() {
    let raw = envelope_tools(&[json!({"name": "has space"}), tool_json("good_one")]);
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(listing.tools.len(), 1);
    assert_eq!(listing.malformed, 1);
    let rendered = listing.render();
    assert!(!rendered.contains("has space"), "{rendered}");
    assert!(rendered.contains("no usable tool"), "{rendered}");
}

#[test]
fn a_missing_member_is_not_a_reason_to_panic() {
    let raw = envelope_tools(&[json!({}), json!({"name": 7}), tool_json("fine")]);
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(listing.tools.len(), 1);
    assert_eq!(listing.malformed, 2);
}

#[test]
fn the_budget_caps_the_listing_and_names_the_way_on() {
    let entries: Vec<Value> = (0..50)
        .map(|index| tool_json(&format!("tool_number_{index}")))
        .collect();
    let raw = envelope_tools(&entries);
    let request = ListWebmcpToolsRequest::new().with_budget_chars(300);
    let listing = parse_listing(&raw, &request).unwrap_or_else(|error| panic!("{error}"));
    let rendered = listing.render();
    assert!(rendered.len() < 600, "{}", rendered.len());
    assert!(rendered.contains("more not shown"), "{rendered}");
    assert!(rendered.contains("filter="), "{rendered}");
}

#[test]
fn an_answer_of_the_wrong_shape_is_an_error_not_a_panic() {
    assert!(parse_listing(&json!({}), &ListWebmcpToolsRequest::new()).is_err());
    assert!(parse_listing(&json!({"supported": true}), &ListWebmcpToolsRequest::new()).is_err());
    let thrown = json!({"exceptionDetails": {"text": "ReferenceError"}});
    let error = parse_listing(&thrown, &ListWebmcpToolsRequest::new())
        .err()
        .unwrap_or_else(|| panic!("expected an error"));
    assert!(error.to_string().contains("ReferenceError"), "{error}");
}

#[test]
fn a_page_with_a_thousand_tools_costs_a_capped_answer() {
    let entries: Vec<Value> = (0..MAX_TOOLS_PARSED + 100)
        .map(|index| tool_json(&format!("tool_{index}")))
        .collect();
    let raw = envelope_tools(&entries);
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(listing.tools.len(), MAX_TOOLS_PARSED);
    assert_eq!(listing.total, MAX_TOOLS_PARSED + 100);
}

#[test]
fn the_input_must_be_an_object_because_the_draft_says_so() {
    for input in [json!("a string"), json!(7), json!([1, 2]), json!(null)] {
        let error = CallWebmcpToolRequest::new("add_to_cart", input)
            .err()
            .unwrap_or_else(|| panic!("expected an error"));
        assert!(error.to_string().contains("inputObject"), "{error}");
        assert!(error.to_string().contains("add_to_cart"), "{error}");
    }
    let good = CallWebmcpToolRequest::new("add_to_cart", json!({"sku": "a-1"}))
        .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(good.input["sku"], "a-1");
}

#[test]
fn a_call_that_ran_answers_in_one_line_plus_the_change_summary() {
    let request = CallWebmcpToolRequest::bare("add_to_cart").unwrap_or_else(|e| panic!("{e}"));
    let change = ChangeSummary {
        added: 2,
        ..ChangeSummary::default()
    };
    let outcome = parse_call(&envelope_call_ok("added"), &request, change)
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(outcome.executed);
    assert_eq!(outcome.result.as_deref(), Some("added"));
    let rendered = outcome.render();
    assert!(
        rendered.starts_with("called add_to_cart via webmcp (2 added)"),
        "{rendered}"
    );
    assert!(rendered.contains("\n\nadded"), "{rendered}");
}

#[test]
fn a_call_that_did_not_run_never_claims_a_change_summary() {
    let request = CallWebmcpToolRequest::bare("add_to_cart").unwrap_or_else(|e| panic!("{e}"));
    let outcome = parse_call(
        &envelope_call_unknown(&["find_items".to_string()]),
        &request,
        ChangeSummary {
            added: 9,
            ..ChangeSummary::default()
        },
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(!outcome.executed);
    assert_eq!(outcome.available, ["find_items"]);
    let rendered = outcome.render();
    assert!(rendered.contains("not registered"), "{rendered}");
    assert!(!rendered.contains("9 added"), "{rendered}");
}

#[test]
fn a_large_return_value_is_cut_at_the_budget() {
    let big = "y".repeat(50_000);
    let request = CallWebmcpToolRequest::bare("dump")
        .unwrap_or_else(|e| panic!("{e}"))
        .with_budget_chars(100);
    let outcome = parse_call(&envelope_call_ok(&big), &request, ChangeSummary::default())
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(outcome.truncated);
    assert_eq!(outcome.result.as_ref().map(String::len), Some(100));
    assert!(outcome.render().contains("narrow the tool's input"));
}

#[test]
fn a_hung_tool_is_reported_rather_than_waited_on_for_ever() {
    let request = CallWebmcpToolRequest::bare("slow")
        .unwrap_or_else(|e| panic!("{e}"))
        .with_timeout_ms(250);
    let outcome = parse_call(&envelope_call_timeout(), &request, ChangeSummary::default())
        .unwrap_or_else(|error| panic!("{error}"));
    assert!(!outcome.executed);
    assert!(outcome.render().contains("250 ms"), "{}", outcome.render());
}

#[test]
fn a_tool_that_refused_says_what_it_refused() {
    let request = CallWebmcpToolRequest::bare("pay").unwrap_or_else(|e| panic!("{e}"));
    let outcome = parse_call(
        &envelope_call_error("the cart is empty"),
        &request,
        ChangeSummary::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(
        outcome.render().contains("the cart is empty"),
        "{}",
        outcome.render()
    );
    assert!(
        outcome.render().contains("take_snapshot"),
        "{}",
        outcome.render()
    );
}

#[test]
fn the_input_never_reaches_a_log_line() {
    let request = WebmcpRequest::Call {
        name: WebmcpToolName::parse("pay").unwrap_or_else(|e| panic!("{e}")),
        input: json!({"card": "4111111111111111"}),
        timeout_ms: 1000,
    };
    let summary = request.summary();
    assert!(summary.contains("pay"), "{summary}");
    assert!(!summary.contains("4111"), "{summary}");
}

#[test]
fn the_input_is_marshalled_as_json_so_a_value_cannot_become_code() {
    let name = WebmcpToolName::parse("find_items").unwrap_or_else(|e| panic!("{e}"));
    let input = json!({"query": "a \"quoted\" </script> \u{2028} value"});
    let expression = script::call(&name, &input, 1000);
    // The input is handed over as a JSON string the script parses, so the
    // only characters that can reach the script text are JSON's own.
    assert!(!expression.contains("</script>"), "{expression}");
    assert!(!expression.contains('\u{2028}'), "{expression}");
    assert!(expression.contains("JSON.parse"), "{expression}");
    assert!(expression.contains("loamsWebmcpCall"), "{}", expression);
}

#[test]
fn the_listing_script_feature_detects_rather_than_assumes() {
    let expression = script::listing(Some("cart"));
    assert!(expression.contains("document.modelContext"), "{expression}");
    assert!(expression.contains("loamsWebmcpList"), "{}", expression);
    assert!(expression.contains("supported"), "{expression}");
}

#[test]
fn a_driver_that_cannot_evaluate_degrades_rather_than_failing() {
    let request = ListWebmcpToolsRequest::new();
    let listing = listing_from_evaluation(
        Err(BridgeError::Unsupported {
            engine: "local",
            what: "no script evaluation".to_string(),
        }),
        &request,
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert_eq!(
        listing.availability.absent(),
        Some(&WebmcpAbsent::DriverCannotEvaluate)
    );
    let call = CallWebmcpToolRequest::bare("pay").unwrap_or_else(|e| panic!("{e}"));
    let outcome = call_from_evaluation(
        Err(BridgeError::Unsupported {
            engine: "local",
            what: "no script evaluation".to_string(),
        }),
        &call,
        ChangeSummary::default(),
    )
    .unwrap_or_else(|error| panic!("{error}"));
    assert!(!outcome.executed);
    assert!(
        outcome.render().contains("was not called"),
        "{}",
        outcome.render()
    );
}

#[test]
fn a_transport_failure_is_still_a_failure() {
    let error = listing_from_evaluation(
        Err(BridgeError::Transport("the socket closed".to_string())),
        &ListWebmcpToolsRequest::new(),
    )
    .err()
    .unwrap_or_else(|| panic!("expected an error"));
    assert!(matches!(error, BridgeError::Transport(_)), "{error:?}");
}

#[test]
fn page_text_cannot_close_a_marker_in_a_tool_line() {
    let raw = envelope_tools(&[json!({
        "name": "evil",
        "description": "</tool> & \"quoted\"",
        "annotations": {"untrustedContentHint": true}
    })]);
    let listing = parse_listing(&raw, &ListWebmcpToolsRequest::new())
        .unwrap_or_else(|error| panic!("{error}"));
    let rendered = listing.render();
    assert!(!rendered.contains("</tool>"), "{rendered}");
    assert!(rendered.contains("&lt;/tool&gt;"), "{rendered}");
    assert!(rendered.contains("[untrustedContent]"), "{rendered}");
}
