//! The v2 WebMCP tools: `list_webmcp_tools` and `call_webmcp_tool` (D570).
//!
//! A page can register tools with `document.modelContext`, so an agent calls a
//! function instead of clicking. That is an **enhancement, never the only
//! path** (D568), and D635 makes the rule load-bearing rather than prudent:
//! WebKit's standards position on WebMCP is closed and `oppose`, so Safari is
//! not a browser that will grow the API later — it is a browser that has said
//! no. Firefox is implementing behind a pref; Chrome is trialling.
//!
//! So every rule here is written for the absent case being the *normal* one:
//!
//! * **Detection is per call, never a capability claim.** The injected script
//!   feature-detects `document.modelContext` itself, which is also the only way
//!   to be right: the API is `[SecureContext]`, and a page can register,
//!   unregister and re-register tools between two calls.
//! * **Absence is an answer, not an error.** [`WebmcpAbsent`] says which of the
//!   reasons it was — the browser has no API, the page is not a secure context,
//!   the `tools` Permissions Policy refuses this origin, the driver cannot
//!   evaluate a script, the page refused — and every message names the way on
//!   (D504 rule 9). The rest of the toolbox is untouched: a Safari page costs a
//!   sentence, not a failed session.
//! * **`call_webmcp_tool` requires a name `list_webmcp_tools` would have
//!   returned.** The name is validated against the draft's rule (1–128
//!   characters of `[A-Za-z0-9_.-]`) before anything is sent, and the page is
//!   asked for its own list before the call, so a stale or invented name comes
//!   back as "not registered; list again" rather than as a silent no-op.
//! * **Outputs obey D504.** A listing is one line per tool with the
//!   description truncated and the whole inside the token budget with a marker
//!   naming the way to continue; a call answers in one line plus the change
//!   summary, then the tool's own return value, also budgeted. Page text is
//!   untrusted (D509): it is escaped and truncated here, and it is never
//!   concatenated into the script that reads it.

#[cfg(test)]
mod tests;
