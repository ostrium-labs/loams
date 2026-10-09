//! The `request_parse` fuzz target's body (Task 3 review I8, fix round 2 N8), also
//! driven on stable by `tests/request_fuzz.rs`. Compiled only with `test-hooks`
//! (the fuzz crate turns it on): no shipped build carries it.
//!
//! Everything the House parses out of a request before a worker sees it, on
//! arbitrary bytes: the query string, the credentials, the session parameters, the
//! content codings, the body decoder under every coding, and the statement scanner
//! — fed in arbitrary pieces, and checked against the whole-text answer.

use crate::auth;
use crate::compress::{self, BodyLimits, CHUNK_BYTES, Decoder, Encoding};
use crate::config::UserMap;
use crate::request::{self, InsertHead, Scanner};

/// Cut points for `data`, taken from its own bytes: arbitrary, reproducible.
fn cuts(data: &[u8]) -> Vec<usize> {
    let mut cuts: Vec<usize> = data
        .iter()
        .step_by(7)
        .map(|b| *b as usize % (data.len() + 1))
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

/// The pieces of `data` between [`cuts`].
fn pieces(data: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut at = 0;
    for cut in cuts(data) {
        out.push(&data[at..cut]);
        at = cut;
    }
    out.push(&data[at..]);
    out
}

/// One fuzz input.
pub fn fuzz_request(data: &[u8]) {
    let text = String::from_utf8_lossy(data);

    // The query string, and what is read from it.
    let params = request::parse_query(&text).unwrap_or_default();
    let headers: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| l.split_once(':'))
        .map(|(n, v)| (n.trim().to_string(), v.trim().to_string()))
        .collect();
    let users = [
        UserMap::dev("default", "", 0, false),
        UserMap::dev("a", "b", 1, true),
    ];
    if let Ok(credentials) = auth::credentials(&headers, &params) {
        let _ = auth::authenticate(&users, &credentials);
    }
    let _ = crate::session::SessionParams::from_params(&params);
    let _ = crate::classify::classify(&text);
    // The deny list (HS1 Task 5): the host-function rewrite keeps every byte
    // outside the calls it replaces, and the explain parser takes any text.
    let host = crate::deny::HostValues {
        display_name: "loams-house",
        timezone: "UTC",
        user: &text,
    };
    let rewritten = crate::deny::rewrite_host_functions(&text, &host);
    assert!(rewritten.len() >= text.len(), "a rewrite only adds");
    let tree = crate::deny::QueryTree::from_explain(&text, Some(&text)).statement(&text);
    let _ = crate::deny::check(&tree);
    // The trailer cut (fix round 1) only ever shortens the text, to a prefix.
    let cut = crate::classify::without_trailer(&text);
    assert!(text.starts_with(cut), "the cut is a prefix");
    let _ = crate::deny::is_denied_setting(&text);
    let limits = crate::settings::SessionLimits::default();
    let known = std::collections::HashSet::from(["max_threads".to_string()]);
    for (name, value) in &params {
        let _ = crate::settings::check(name, value, &limits, &known);
    }
    let _ = compress::content_encoding(Some(&text));
    let _ = compress::accepted(Some(&text));
    let _ = request::is_read(&text);
    let _ = request::split_format(&text);

    // The body decoder, under every coding, fed in arbitrary pieces: it may refuse
    // the bytes, but never panics and never hands out a piece over CHUNK_BYTES.
    for coding in [
        None,
        Some(Encoding::Gzip),
        Some(Encoding::Deflate),
        Some(Encoding::Zstd),
    ] {
        let Ok(mut decoder) = Decoder::new(coding, BodyLimits::default()) else {
            continue;
        };
        let mut failed = false;
        for piece in pieces(data) {
            decoder.push(piece);
            loop {
                match decoder.next_piece() {
                    Ok(Some(out)) => assert!(out.len() <= CHUNK_BYTES),
                    Ok(None) => break,
                    Err(_) => {
                        failed = true;
                        break;
                    }
                }
            }
            if failed {
                break;
            }
        }
        if !failed {
            decoder.end();
            while let Ok(Some(out)) = decoder.next_piece() {
                assert!(out.len() <= CHUNK_BYTES);
            }
        }
    }

    // The scanner, in arbitrary pieces, equals the scanner over the whole text.
    let mut whole = Scanner::collecting();
    whole.finish(data);
    let mut piecewise = Scanner::collecting();
    let mut seen = 0;
    let mut early: Option<(usize, InsertHead)> = None;
    let mut streaming = Scanner::new();
    for piece in pieces(data) {
        seen += piece.len();
        piecewise.advance(&data[..seen]);
        streaming.advance(&data[..seen]);
        if early.is_none() {
            let head = streaming.insert_head(&data[..seen], false);
            if matches!(head, InsertHead::Insert { .. }) {
                early = Some((seen, head));
            }
        }
    }
    piecewise.finish(data);
    assert_eq!(
        whole.tokens(),
        piecewise.tokens(),
        "scanning in pieces changed the tokens"
    );
    assert_eq!(
        whole.insert_head(data, true),
        piecewise.insert_head(data, true)
    );

    // An INSERT head found early, on a prefix, is the one the whole text plans.
    if let Some((
        _,
        InsertHead::Insert {
            statement,
            format,
            data_start,
        },
    )) = early
    {
        match whole.insert_head(data, true) {
            InsertHead::Insert {
                statement: s,
                format: f,
                data_start: d,
            } => assert_eq!((statement.clone(), format.clone(), data_start), (s, f, d)),
            other => panic!("early {statement:?}/{format:?}, whole {other:?}"),
        }
        let plan = request::plan(data, false);
        let input = plan.input.expect("the whole text plans the same INSERT");
        assert_eq!((input.insert, input.format), (statement, format));
    }
    let _ = request::plan(data, true);
}
