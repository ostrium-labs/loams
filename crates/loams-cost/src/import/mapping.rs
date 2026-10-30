//! Mapping rules (design §84 §6.3, D2355): a bill line goes to the pool and
//! kind of the first rule it matches. Built-in rules come last and read the
//! tags `loams-pool` and `loams-kind`. A line that matches nothing is
//! unmapped: reported, never allocated, never dropped.

use crate::import::{BillLine, ChargeCategory};
use crate::inputs::{LineMatch, MappingRule};
use crate::{CostError, PoolId, ResourceKind};

/// The pool that taxes, credits and adjustments with no resource and no
/// matching rule go to (kind `fixed`, so their lines are `shared`).
pub const SHARED_POOL: &str = "_shared";

/// `*` matches any run of characters, `?` one character; everything else
/// is literal. Case-sensitive.
pub fn glob(pattern: &str, text: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut mark) = (None::<usize>, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == t[ti]) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = ti;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            ti = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

fn field_matches(pattern: &Option<String>, value: Option<&str>) -> bool {
    match pattern {
        None => true,
        Some(p) => value.is_some_and(|v| glob(p, v)),
    }
}

/// Whether `line` matches every field `m` gives.
pub fn matches(m: &LineMatch, line: &BillLine) -> bool {
    field_matches(&m.resource, line.resource.as_deref())
        && field_matches(&m.service, line.service.as_deref())
        && field_matches(&m.sku, line.sku.as_deref())
        && field_matches(&m.region, line.region.as_deref())
        && field_matches(&m.category, Some(line.category.as_str()))
        && m.tags
            .iter()
            .all(|(k, p)| line.tags.get(k).is_some_and(|v| glob(p, v)))
}

/// Expands `{tags.<key>}` and `{tags.<key>|default}` in `template`. `None`
/// when a tag is missing and has no default.
pub fn expand(template: &str, line: &BillLine) -> Option<String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let close = rest[open..].find('}')? + open;
        let inner = &rest[open + 1..close];
        let (name, default) = match inner.split_once('|') {
            Some((n, d)) => (n, Some(d)),
            None => (inner, None),
        };
        let key = name.strip_prefix("tags.")?;
        out.push_str(line.tags.get(key).map(String::as_str).or(default)?);
        rest = &rest[close + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// The built-in rule: the tags `loams-pool` and `loams-kind` (default `cpu`).
pub fn builtin_rules() -> Vec<MappingRule> {
    vec![MappingRule {
        matcher: LineMatch {
            tags: [("loams-pool".to_string(), "*".to_string())].into(),
            ..Default::default()
        },
        pool: "{tags.loams-pool}".into(),
        kind: "{tags.loams-kind|cpu}".into(),
    }]
}

/// How a line was mapped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Mapped {
    /// To this pool and kind, by rule `rule` (`None` for a built-in rule).
    Pool {
        pool: PoolId,
        kind: ResourceKind,
        rule: Option<usize>,
    },
    /// A tax, credit or adjustment with no resource: to [`SHARED_POOL`].
    Shared,
    /// Nothing matched.
    Unmapped,
}

/// Maps `line` with `rules`, then the built-in rules. A matching rule whose
/// template expands to an invalid pool or kind is an error (the operator's
/// rule or tags are wrong, and silence would hide cost).
pub fn map_line(rules: &[MappingRule], line: &BillLine) -> Result<Mapped, CostError> {
    let builtin = builtin_rules();
    let all = rules
        .iter()
        .enumerate()
        .map(|(i, r)| (Some(i), r))
        .chain(builtin.iter().map(|r| (None, r)));
    for (index, rule) in all {
        if !matches(&rule.matcher, line) {
            continue;
        }
        let (Some(pool), Some(kind)) = (expand(&rule.pool, line), expand(&rule.kind, line)) else {
            continue;
        };
        let where_ = match index {
            Some(i) => format!("mappings[{i}]"),
            None => "built-in mapping".to_string(),
        };
        let pool = PoolId::new(pool)
            .map_err(|e| CostError::invalid(where_.clone(), format!("line {}: {e}", line.line)))?;
        let kind = ResourceKind::parse(&kind)
            .map_err(|e| CostError::invalid(where_, format!("line {}: {e}", line.line)))?;
        return Ok(Mapped::Pool {
            pool,
            kind,
            rule: index,
        });
    }
    let sharable = matches!(
        line.category,
        ChargeCategory::Tax | ChargeCategory::Credit | ChargeCategory::Adjustment
    );
    if sharable && line.resource.as_deref().is_none_or(str::is_empty) {
        return Ok(Mapped::Shared);
    }
    Ok(Mapped::Unmapped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::tests::line;

    #[test]
    fn glob_basics() {
        assert!(glob("*", ""));
        assert!(glob("i-*", "i-0abc"));
        assert!(glob(
            "*/loams-house-worker*",
            "/sys/fs/cgroup/x/loams-house-worker-3"
        ));
        assert!(glob("a?c", "abc"));
        assert!(!glob("a?c", "ac"));
        assert!(!glob("i-*", "vol-1"));
        assert!(glob("*a*b*", "xxaxxbxx"));
    }

    #[test]
    fn tag_template_with_default() {
        let mut l = line();
        l.tags.insert("loams-pool".into(), "eu/compute".into());
        assert_eq!(
            expand("{tags.loams-pool}", &l).as_deref(),
            Some("eu/compute")
        );
        assert_eq!(expand("{tags.loams-kind|cpu}", &l).as_deref(), Some("cpu"));
        assert_eq!(expand("{tags.missing}", &l), None);
        assert_eq!(
            expand("x-{tags.loams-pool}-y", &l).as_deref(),
            Some("x-eu/compute-y")
        );
    }

    #[test]
    fn mapping_first_match_wins() {
        let mut l = line();
        l.service = Some("Amazon Simple Storage Service".into());
        l.tags.insert("loams-pool".into(), "eu/compute".into());
        let rules = vec![
            MappingRule {
                matcher: LineMatch {
                    service: Some("Amazon Simple*".into()),
                    ..Default::default()
                },
                pool: "eu/bucket".into(),
                kind: "storage".into(),
            },
            MappingRule {
                matcher: LineMatch::default(),
                pool: "eu/other".into(),
                kind: "cpu".into(),
            },
        ];
        assert_eq!(
            map_line(&rules, &l).expect("map"),
            Mapped::Pool {
                pool: PoolId::new("eu/bucket").expect("p"),
                kind: ResourceKind::Storage,
                rule: Some(0)
            }
        );
        // No operator rule matches: the built-in tag rule does.
        assert_eq!(
            map_line(&rules[..1].iter().take(0).cloned().collect::<Vec<_>>(), &l).expect("map"),
            Mapped::Pool {
                pool: PoolId::new("eu/compute").expect("p"),
                kind: ResourceKind::Cpu,
                rule: None
            }
        );
    }

    #[test]
    fn tax_without_resource_is_shared() {
        let mut l = line();
        l.category = ChargeCategory::Tax;
        l.resource = None;
        assert_eq!(map_line(&[], &l).expect("map"), Mapped::Shared);
        l.resource = Some("i-1".into());
        assert_eq!(map_line(&[], &l).expect("map"), Mapped::Unmapped);
    }

    #[test]
    fn bad_template_value_is_an_error() {
        let mut l = line();
        l.tags.insert("loams-pool".into(), "eu/compute".into());
        l.tags.insert("loams-kind".into(), "disk".into());
        assert!(map_line(&[], &l).is_err());
    }
}
