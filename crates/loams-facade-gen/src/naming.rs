//! The naming rules the renderers share: turning a proto name into an
//! identifier in a target language (design §44 §7.1).
//!
//! The proto names are the source of truth; every renderer converts rather than
//! inventing. `snake_case` and `PascalCase` calls convert to the language's
//! case, and a module name (`admin.org`, `instance`) becomes a type name
//! (`AdminOrg`, `Instance`) that cannot collide with another module's.
//!
//! Each renderer keeps its own copy of anything that is specific to its
//! language — Python's `snake_case` methods need no conversion at all, Go's
//! exported fields need `PascalCase`, Rust's struct fields stay `snake_case`
//! — because a shared helper that all three call with a flag is harder to read
//! than three small ones. What is genuinely the same for all of them lives
//! here.

/// The last segment of a fully qualified proto name:
/// `loams.instance.v1.GetInstanceRequest` is `GetInstanceRequest`.
///
/// protoc writes a type reference with a leading dot
/// (`.loams.instance.v1.GetInstanceRequest`), so this ignores one.
pub fn leaf(full: &str) -> &str {
    full.trim_start_matches('.')
        .rsplit('.')
        .next()
        .unwrap_or(full)
}

/// The proto package of a fully qualified message name, which is where the
/// package map is keyed: `.loams.instance.v1.GetInstanceRequest` is
/// `loams.instance.v1`.
pub fn package_of(full: &str) -> &str {
    let full = full.trim_start_matches('.');
    match full.rfind('.') {
        Some(at) => &full[..at],
        None => "",
    }
}

/// A module name as a type name: `admin.org` is `AdminOrg`, `instance` is
/// `Instance`. The separators a module name may carry are dots, underscores
/// and dashes; each starts a new word.
pub fn type_name(name: &str) -> String {
    pascal(name)
}

/// `snake_case` to `PascalCase`, which is what Go names its methods and
/// exported fields (design §44 §7.1).
pub fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut upper = true;
    for character in name.chars() {
        if character == '.' || character == '_' || character == '-' {
            upper = true;
            continue;
        }
        if upper {
            out.extend(character.to_uppercase());
            upper = false;
        } else {
            out.push(character);
        }
    }
    out
}

/// A Go field or variable name: `next_page_token` is `NextPageToken`, and a
/// name that already starts with a capital keeps it.
pub fn exported(name: &str) -> String {
    pascal(name)
}

/// `camelCase` or `PascalCase` to `snake_case`, which is what Python's and Rust's
/// method names are (design §44 §7.1).
///
/// The annotations on the protos carry **camelCase** names — `GetInstance` is
/// annotated `name: "getInstance"`, because the first language written against
/// them was TypeScript — so every renderer converts to its own case rather than
/// using the name verbatim. A name that is already `snake_case` is unchanged,
/// which is what keeps the conversion idempotent.
///
/// A run of capitals is kept together except for its last letter, so `whoAmI`
/// becomes `who_am_i` and not `who_a_m_i`.
pub fn snake(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    let mut out = String::new();
    for (index, character) in chars.iter().enumerate() {
        if character.is_uppercase() {
            let after_lower_or_digit =
                index > 0 && (chars[index - 1].is_lowercase() || chars[index - 1].is_ascii_digit());
            let before_lower = chars.get(index + 1).is_some_and(|next| next.is_lowercase());
            if index > 0 && !out.ends_with('_') && (after_lower_or_digit || before_lower) {
                out.push('_');
            }
            out.extend(character.to_lowercase());
        } else {
            out.push(*character);
        }
    }
    out
}

/// The identifier a renderer that imports message types by their bare name
/// (TypeScript, Python) gives one message: its leaf, unless another package's
/// message the facade also names has the same leaf. Then the first package in
/// sorted order keeps the bare leaf and every other one is prefixed with its
/// own package (`loams.sql.v1.QueryRequest` beside `loams.live.v1.QueryRequest`
/// is `SqlV1QueryRequest`), so the two imports cannot shadow each other.
pub fn local_name(model: &crate::Model, full: &str) -> String {
    let full = full.trim_start_matches('.');
    let name = leaf(full);
    let package = package_of(full);
    let mut owners: Vec<&str> = model
        .calls()
        .flat_map(|call| [call.input.as_str(), call.output.as_str()])
        .map(|other| other.trim_start_matches('.'))
        .filter(|other| leaf(other) == name)
        .map(package_of)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    if owners.first().is_none_or(|first| *first == package) {
        return name.to_owned();
    }
    let prefix = package.strip_prefix("loams.").unwrap_or(package);
    format!("{}{name}", pascal(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_module_name_becomes_a_type_name() {
        assert_eq!(type_name("admin.org"), "AdminOrg");
        assert_eq!(type_name("instance"), "Instance");
        assert_eq!(type_name("search-vector"), "SearchVector");
    }

    #[test]
    fn a_proto_name_becomes_a_wire_name() {
        assert_eq!(
            leaf("loams.instance.v1.GetInstanceRequest"),
            "GetInstanceRequest"
        );
        assert_eq!(leaf("GetInstanceRequest"), "GetInstanceRequest");
    }

    /// protoc writes a type reference with a leading dot, so every name the
    /// plugin reads has one and every name a renderer looks up must not.
    #[test]
    fn a_leading_dot_is_ignored() {
        assert_eq!(
            leaf(".loams.instance.v1.GetInstanceRequest"),
            "GetInstanceRequest"
        );
        assert_eq!(
            package_of(".loams.instance.v1.GetInstanceRequest"),
            "loams.instance.v1"
        );
        assert_eq!(
            package_of("loams.instance.v1.GetInstanceRequest"),
            "loams.instance.v1"
        );
        assert_eq!(package_of("GetInstanceRequest"), "");
    }

    /// The annotations are camelCase, so this conversion is what makes the
    /// Python and Rust surfaces idiomatic rather than camel-cased.
    #[test]
    fn a_camel_case_name_becomes_snake_case() {
        assert_eq!(snake("getInstance"), "get_instance");
        assert_eq!(snake("whoAmI"), "who_am_i");
        assert_eq!(snake("GetInstance"), "get_instance");
        assert_eq!(snake("search"), "search");
        assert_eq!(snake("next_page_token"), "next_page_token");
    }

    #[test]
    fn a_call_name_becomes_pascal_case() {
        assert_eq!(pascal("get_instance"), "GetInstance");
        assert_eq!(pascal("who_am_i"), "WhoAmI");
        assert_eq!(pascal("search"), "Search");
        assert_eq!(exported("next_page_token"), "NextPageToken");
    }
}
