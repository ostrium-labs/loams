//! The injected script: the only thing that touches `document.modelContext`.
//!
//! Two rules shape it.
//!
//! * **It feature-detects, it never assumes.** `document.modelContext` is
//!   `[SecureContext]` and absent from Safari outright (D635), so the first
//!   thing each script does is look, and absence is reported as an answer with
//!   a reason rather than as an exception. A page is free to replace the
//!   script it was handed, which is why the Rust side treats the whole reply as
//!   untrusted data (D509) and the parser checks its shape.
//! * **It answers JSON and never throws.** `eval_with_callback` drops a thrown
//!   exception on Windows (design §37 §18.14.3), so every path returns a JSON
//!   string, and `awaitPromise` carries the promise for the one call that
//!   returns one (`getTools`). A failure of the script itself is therefore
//!   visible as `exceptionDetails`, which the parser reports rather than as a
//!   silent `null`.
//!
//! The two entry points are named functions — `loamsWebmcpList` and
//! `loamsWebmcpCall` — so a page, a log line or a conformance test can tell
//! which script ran and answer each one separately.

use serde_json::Value;

use super::WebmcpToolName;

/// The version tag every answer carries, so a reply from a newer bridge's
/// script is distinguishable from this one's.
const VERSION: &str = "webmcp/1";

/// The JavaScript both scripts share: answering an absence, cloning a value out
/// of the page, turning a tool's return value into text, and naming an error.
///
/// `absent` is what keeps the two scripts telling the same story: a page with no
/// `document.modelContext` answers a listing and a call with the *same* envelope,
/// so the bridge reads one typed fact out of both rather than a tool failure on
/// one and an absence on the other.
fn helpers() -> String {
    format!(
        r#"function absent(reason) {{
      return JSON.stringify({{ loams: '{VERSION}', supported: false, reason: reason }});
    }}
    function clone(value) {{
      if (value === undefined || value === null) {{ return null; }}
      try {{ return JSON.parse(JSON.stringify(value)); }} catch (error) {{ return null; }}
    }}
    function stringify(value) {{
      if (typeof value === 'string') {{ return value; }}
      if (value === undefined || value === null) {{ return ''; }}
      try {{ return JSON.stringify(value); }} catch (error) {{ return String(value); }}
    }}
    function nameOf(error) {{
      if (!error) {{ return 'the page gave no reason'; }}
      var name = error.name ? String(error.name) : '';
      var message = error.message ? String(error.message) : '';
      if (name && message) {{ return name + ': ' + message; }}
      return name || message || 'the page gave no reason';
    }}"#,
        VERSION = VERSION,
    )
}

/// List the page's registered tools, keeping only those whose name contains
/// `filter` (case-insensitively; no filter, or an empty one, keeps everything).
pub(crate) fn listing(filter: Option<&str>) -> String {
    let filter = json_literal(filter.unwrap_or_default());
    format!(
        r#"(function loamsWebmcpList() {{
  try {{
    if (typeof document === 'undefined' || !document) {{
      return absent('not-exposed');
    }}
    if (window.isSecureContext === false) {{
      return absent('insecure-context');
    }}
    var context = document.modelContext;
    if (!context || typeof context.getTools !== 'function') {{
      return absent('not-exposed');
    }}
    var wanted = {filter}.toLowerCase();
    return Promise.resolve(context.getTools()).then(function (tools) {{
      if (!tools || typeof tools.length !== 'number') {{
        return absent('not-exposed');
      }}
      var out = [];
      for (var index = 0; index < tools.length; index += 1) {{
        var tool;
        try {{
          tool = tools[index];
          if (!tool || typeof tool.name !== 'string') {{
            out.push({{ name: null }});
            continue;
          }}
        }} catch (error) {{
          // A getter that throws is one tool the page will not describe.
          out.push({{ name: null }});
          continue;
        }}
        if (wanted !== '' && tool.name.toLowerCase().indexOf(wanted) === -1) {{
          continue;
        }}
        out.push({{
          name: tool.name,
          title: typeof tool.title === 'string' ? tool.title : null,
          description: typeof tool.description === 'string' ? tool.description : '',
          // A schema that cannot be cloned is reported as absent rather than
          // failing the whole listing: the name is what a call needs.
          inputSchema: clone(tool.inputSchema),
          annotations: clone(tool.annotations)
        }});
      }}
      return JSON.stringify({{ loams: '{VERSION}', supported: true, tools: out }});
    }}, function (error) {{
      return absent(nameOf(error));
    }});
  }} catch (error) {{
    return absent(nameOf(error));
  }}
  {helpers}
}})()"#,
        filter = filter,
        VERSION = VERSION,
        helpers = helpers(),
    )
}

/// Execute `name` with `input`, waiting at most `timeout_ms` for it.
pub(crate) fn call(name: &WebmcpToolName, input: &Value, timeout_ms: u64) -> String {
    let name = json_literal(name.as_str());
    let input = json_literal(&serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()));
    format!(
        r#"(function loamsWebmcpCall() {{
  var timer = null;
  function clearTimer() {{
    if (timer !== null) {{
      try {{
        clearTimeout(timer);
      }} catch (error) {{
        // Nothing useful to do with a timer that will not clear.
      }}
      timer = null;
    }}
  }}
  try {{
    // The same detection as the listing, and the same envelope: a call against
    // a page with no `document.modelContext` reports the absence rather than a
    // tool that failed, because no tool was ever reachable.
    if (typeof document === 'undefined' || !document) {{
      return absent('not-exposed');
    }}
    if (window.isSecureContext === false) {{
      return absent('insecure-context');
    }}
    if (!document.modelContext) {{
      return absent('not-exposed');
    }}
    var context = document.modelContext;
    if (typeof context.getTools !== 'function' || typeof context.executeTool !== 'function') {{
      return absent('not-exposed');
    }}
    var wanted = {name};
    var input = JSON.parse({input});
    return Promise.resolve(context.getTools()).then(function (tools) {{
      var names = [];
      var found = null;
      for (var index = 0; index < (tools ? tools.length : 0); index += 1) {{
        try {{
          var tool = tools[index];
          if (tool && typeof tool.name === 'string') {{
            names.push(tool.name);
            if (tool.name === wanted) {{
              found = tool;
            }}
          }}
        }} catch (error) {{
          // As above: one undescribable tool costs one tool.
        }}
      }}
      if (!found) {{
        return JSON.stringify({{ loams: '{VERSION}', state: 'unknown-tool', tools: names }});
      }}
      // A tool is page code and may never return, so the wait is bounded here
      // as well as in the transport. The AbortSignal is what the draft's
      // `execute` receives (the registration-time signal is a different thing
      // and unregisters instead, D635), and every settling path clears the
      // timer so a finished call does not hold the page's event loop open.
      var controller = typeof AbortController === 'function' ? new AbortController() : null;
      var guard = new Promise(function (resolve, reject) {{
        timer = setTimeout(function () {{
          if (controller) {{
            try {{
              controller.abort();
            }} catch (error) {{
              // Aborting is best-effort; the timeout still rejects.
            }}
          }}
          reject(new Error('webmcp-timeout'));
        }}, {timeout_ms});
      }});
      var options = controller ? {{ signal: controller.signal }} : {{}};
      var settled;
      try {{
        // `executeTool` is page code: it can throw synchronously, before it
        // ever returns a promise. Calling it bare would let that throw escape
        // this fulfilment callback — where neither the rejection handler below
        // nor the outer `catch` could see it — and leave the timeout running.
        settled = Promise.resolve(context.executeTool(found, input, options));
      }} catch (error) {{
        clearTimer();
        return JSON.stringify({{ loams: '{VERSION}', state: 'error', reason: nameOf(error) }});
      }}
      return Promise.race([settled, guard]).then(function (text) {{
        clearTimer();
        return JSON.stringify({{ loams: '{VERSION}', state: 'ok', text: stringify(text) }});
      }}, function (error) {{
        clearTimer();
        if (error && error.message === 'webmcp-timeout') {{
          return JSON.stringify({{ loams: '{VERSION}', state: 'timeout' }});
        }}
        return JSON.stringify({{ loams: '{VERSION}', state: 'error', reason: nameOf(error) }});
      }});
    }}, function (error) {{
      // A refused `getTools()` never reached a tool, and the Permissions Policy
      // refusal arrives here, so it is the absence rather than a failed call.
      clearTimer();
      return absent(nameOf(error));
    }});
  }} catch (error) {{
    clearTimer();
    return absent(nameOf(error));
  }}
  {helpers}
}})()"#,
        name = name,
        input = input,
        timeout_ms = timeout_ms,
        VERSION = VERSION,
        helpers = helpers(),
    )
}

/// A JSON string literal for `value`.
///
/// This is the injection boundary: every value the bridge puts into the page
/// goes through here, so a tool name or an input value can only ever be a
/// string inside the script. `<` and `>` are escaped too, so nothing in the
/// payload can close a `<script>` element if a host ever inlines the script, and the two
/// Unicode line terminators are escaped because they are legal in JSON but
/// illegal in a JavaScript string literal.
fn json_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for character in value.chars() {
        match character {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '<' => out.push_str("\\u003c"),
            '>' => out.push_str("\\u003e"),
            '\u{2028}' => out.push_str("\\u2028"),
            '\u{2029}' => out.push_str("\\u2029"),
            other if (other as u32) < 0x20 || other as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_character_of_a_name_stays_a_string() {
        let raw = "quote\" back\\slash <tag> \u{2028} tab\t";
        let literal = json_literal(raw);
        assert!(!literal.contains('<'), "{literal}");
        assert!(!literal.contains('\u{2028}'), "{literal}");
        let parsed: Value = serde_json::from_str(&literal).unwrap_or_default();
        assert_eq!(parsed.as_str(), Some(raw));
    }

    #[test]
    fn the_call_script_parses_its_input_rather_than_pasting_it() {
        let name = WebmcpToolName::parse("find_items").unwrap_or_else(|error| panic!("{error}"));
        let input = json!({"query": "a \"quoted\" <script> value"});
        let expression = call(&name, &input, 1000);
        assert!(expression.contains("JSON.parse"), "{expression}");
        assert!(!expression.contains("<script>"), "{expression}");
        assert!(expression.contains("executeTool"), "{expression}");
        assert!(expression.contains("loamsWebmcpCall"), "{}", expression);
    }

    #[test]
    fn the_listing_script_names_itself_and_the_api_it_needs() {
        let expression = listing(Some("cart"));
        assert!(expression.contains("loamsWebmcpList"), "{}", expression);
        assert!(
            expression.contains("document.modelContext"),
            "{}",
            expression
        );
        assert!(expression.contains("getTools"), "{expression}");
        assert!(expression.contains("isSecureContext"), "{expression}");
        assert!(expression.contains("\"cart\""), "{expression}");
    }

    #[test]
    fn the_call_script_reports_an_absent_api_as_an_absence_and_not_a_tool_failure() {
        let name = WebmcpToolName::parse("add_to_cart").unwrap_or_else(|error| panic!("{error}"));
        let expression = call(&name, &json!({}), 1000);
        // The call and the listing report the same fact the same way, so
        // `parse_call` reads a typed absence rather than a tool that failed.
        assert!(expression.contains("supported: false"), "{expression}");
        assert!(expression.contains("insecure-context"), "{expression}");
        assert!(
            !expression.contains("state: 'error', reason: 'not-exposed'"),
            "an absent document.modelContext must not answer as a tool failure: {expression}"
        );
    }

    #[test]
    fn a_get_tools_refusal_on_a_call_is_an_absence_rather_than_a_tool_failure() {
        let name = WebmcpToolName::parse("pay").unwrap_or_else(|error| panic!("{error}"));
        let expression = call(&name, &json!({}), 1000);
        // The rejection of `getTools()` is what the Permissions Policy refusal
        // arrives as, so it must be the absent envelope.
        assert!(expression.contains("absent(nameOf(error))"), "{expression}");
    }

    #[test]
    fn both_scripts_carry_the_same_helpers() {
        for expression in [
            listing(None),
            call(
                &WebmcpToolName::parse("t").unwrap_or_else(|error| panic!("{error}")),
                &json!({}),
                10,
            ),
        ] {
            assert!(
                expression.contains("function nameOf(error)"),
                "{}",
                expression
            );
            assert!(
                expression.contains("function clone(value)"),
                "{}",
                expression
            );
        }
    }
}
