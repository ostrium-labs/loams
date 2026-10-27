//! Secrets out of every message that leaves the daemon (§50 §12): each secret in
//! every form an upstream could echo it — raw, URL-encoded, form-encoded, base64 and
//! inside an HTTP Basic pair with any other field.

use base64::Engine as _;

/// What a secret becomes.
pub const MASK: &str = "[redacted]";

fn b64(s: &str) -> [String; 2] {
    let full = base64::engine::general_purpose::STANDARD.encode(s);
    let trimmed = full.trim_end_matches('=').to_owned();
    [full, trimmed]
}

/// Every form of `secrets`; `plain` are the other configured fields (a Basic user name).
pub fn secret_forms(secrets: &[String], plain: &[String]) -> Vec<String> {
    let mut forms: Vec<String> = Vec::new();
    let mut add = |f: String| {
        if !f.is_empty() && !forms.contains(&f) {
            forms.push(f);
        }
    };
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        add(s.clone());
        add(crate::client::encode_segment(s));
        add(url::form_urlencoded::byte_serialize(s.as_bytes()).collect());
        for f in b64(s) {
            add(f);
        }
        for u in plain.iter().chain(secrets.iter()) {
            if u.is_empty() || u == s {
                continue;
            }
            for f in b64(&format!("{u}:{s}"))
                .into_iter()
                .chain(b64(&format!("{s}:{u}")))
            {
                add(f);
            }
        }
    }
    forms
}

/// `text` with every form replaced by [`MASK`], longest first.
pub fn redact(text: &str, forms: &[String]) -> String {
    let mut sorted: Vec<&String> = forms.iter().filter(|f| !f.is_empty()).collect();
    sorted.sort_by_key(|f| std::cmp::Reverse(f.len()));
    let mut out = text.to_owned();
    for f in sorted {
        out = out.replace(f.as_str(), MASK);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_form_is_redacted() {
        let secret = "tok-S3CRET/+x".to_owned();
        let forms = secret_forms(std::slice::from_ref(&secret), &["bot@example.com".into()]);
        let basic =
            base64::engine::general_purpose::STANDARD.encode("bot@example.com:tok-S3CRET/+x");
        for text in [
            "tok-S3CRET/+x".to_owned(),
            "tok-S3CRET%2F%2Bx".to_owned(),
            basic.clone(),
            basic.trim_end_matches('=').to_owned(),
        ] {
            let out = redact(&format!("before {text} after"), &forms);
            assert!(!out.contains("S3CRET"), "{out}");
            assert!(!out.contains(&basic), "{out}");
        }
        assert!(redact("nothing here", &forms) == "nothing here");
    }
}
