//! What each facade route needs (D2016), in IA2's deny-by-default style: a route declares one
//! `can_*` relation on one object through [`loams_authz::http`], and an undeclared route is
//! refused.
//!
//! An instance belongs to one Loams namespace (ENG81 R2). `deploy/openfga/core.fga` has an
//! `engine` type (`can_view`, `can_operate`, `can_admin`, `can_connect` from its environment,
//! R18/D1919), but until the facade writes `engine:<id>#environment` tuples every check is the
//! matching relation on `environment:<ns>`:
//!
//! | Route | On `environment:<ns>` | Same as on `engine:<id>` |
//! |---|---|---|
//! | `GET /v1/engines` | authenticated | |
//! | list, get, usage, list sessions and jobs | `can_read` | `can_view` |
//! | suspend, resume, SQL, create or close a session, submit or cancel a job | `can_write` | `can_operate` |
//! | create, `PATCH` (scale or upgrade), delete | `can_admin`, strict (no cache) | `can_admin` |
//!
//! `PATCH` needs `can_admin` for a scale too, because the route table cannot see the body.

use axum::http::Method;
use loams_authz::ObjectRef;
use loams_authz::http::{Declared, Need, segments};

/// Reads (`engine#can_view`).
pub const CAN_VIEW: &str = "can_read";
/// Lifecycle and use (`engine#can_operate`).
pub const CAN_OPERATE: &str = "can_write";
/// Create, change and delete (`engine#can_admin`).
pub const CAN_ADMIN: &str = "can_admin";

fn check(relation: &str, namespace: &str, strict: bool) -> Option<Declared> {
    let need = Need::new(relation, ObjectRef::new("environment", namespace)).ok()?;
    Some(Declared::Check(if strict { need.strict() } else { need }))
}

/// The route table for [`loams_authz::http::Enforcer`]. `None` (deny) for anything that is not
/// a facade route.
pub fn declare(method: &Method, path: &str) -> Option<Declared> {
    let seg = segments(path)?;
    let seg: Vec<&str> = seg.iter().map(String::as_str).collect();
    match seg.as_slice() {
        ["v1", "engines"] if *method == Method::GET => Some(Declared::Authenticated),
        ["v1", "namespaces", p, "engines"] => match *method {
            Method::GET => check(CAN_VIEW, p, false),
            Method::POST => check(CAN_ADMIN, p, true),
            _ => None,
        },
        ["v1", "namespaces", p, "engines", _id] => match *method {
            Method::GET => check(CAN_VIEW, p, false),
            Method::PATCH | Method::DELETE => check(CAN_ADMIN, p, true),
            _ => None,
        },
        ["v1", "namespaces", p, "engines", _id, action] => match (method, *action) {
            (&Method::POST, "suspend" | "resume" | "sql" | "sessions" | "jobs" | "schedules") => {
                check(CAN_OPERATE, p, false)
            }
            (&Method::GET, "usage" | "sessions" | "jobs" | "schedules") => {
                check(CAN_VIEW, p, false)
            }
            _ => None,
        },
        [
            "v1",
            "namespaces",
            p,
            "engines",
            _id,
            "sessions" | "schedules",
            _s,
        ] if *method == Method::DELETE => check(CAN_OPERATE, p, false),
        ["v1", "namespaces", p, "engines", _id, "jobs", _j] if *method == Method::GET => {
            check(CAN_VIEW, p, false)
        }
        ["v1", "namespaces", p, "engines", _id, "jobs", _j, "logs"] if *method == Method::GET => {
            check(CAN_VIEW, p, false)
        }
        ["v1", "namespaces", p, "engines", _id, "jobs", _j, "cancel"]
            if *method == Method::POST =>
        {
            check(CAN_OPERATE, p, false)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use loams_authz::Consistency;

    use super::*;

    fn need(m: Method, path: &str) -> Option<(String, String, Consistency)> {
        match declare(&m, path)? {
            Declared::Check(n) => Some((
                n.relation().to_owned(),
                n.object().to_string(),
                n.consistency(),
            )),
            Declared::Authenticated => Some((
                "authenticated".into(),
                String::new(),
                Consistency::MinimizeLatency,
            )),
            Declared::Public => {
                Some(("public".into(), String::new(), Consistency::MinimizeLatency))
            }
        }
    }

    #[test]
    fn every_route_is_declared() {
        let base = "/v1/namespaces/p1/engines";
        let id = format!("{base}/eng_1");
        let cases = [
            (Method::GET, "/v1/engines".to_owned(), "authenticated"),
            (Method::GET, base.to_owned(), CAN_VIEW),
            (Method::POST, base.to_owned(), CAN_ADMIN),
            (Method::GET, id.clone(), CAN_VIEW),
            (Method::PATCH, id.clone(), CAN_ADMIN),
            (Method::DELETE, id.clone(), CAN_ADMIN),
            (Method::POST, format!("{id}/suspend"), CAN_OPERATE),
            (Method::POST, format!("{id}/resume"), CAN_OPERATE),
            (Method::GET, format!("{id}/usage"), CAN_VIEW),
            (Method::POST, format!("{id}/sql"), CAN_OPERATE),
            (Method::GET, format!("{id}/sessions"), CAN_VIEW),
            (Method::POST, format!("{id}/sessions"), CAN_OPERATE),
            (Method::DELETE, format!("{id}/sessions/s1"), CAN_OPERATE),
            (Method::GET, format!("{id}/jobs"), CAN_VIEW),
            (Method::POST, format!("{id}/jobs"), CAN_OPERATE),
            (Method::POST, format!("{id}/jobs/j1/cancel"), CAN_OPERATE),
            (Method::GET, format!("{id}/jobs/j1"), CAN_VIEW),
            (Method::GET, format!("{id}/jobs/j1/logs"), CAN_VIEW),
            (Method::GET, format!("{id}/schedules"), CAN_VIEW),
            (Method::POST, format!("{id}/schedules"), CAN_OPERATE),
            (
                Method::DELETE,
                format!("{id}/schedules/nightly"),
                CAN_OPERATE,
            ),
        ];
        for (m, path, want) in cases {
            let (rel, obj, consistency) =
                need(m.clone(), &path).unwrap_or_else(|| panic!("{m} {path} is undeclared"));
            assert_eq!(rel, want, "{m} {path}");
            if want != "authenticated" {
                assert_eq!(obj, "environment:p1", "{m} {path}");
            }
            let strict = want == CAN_ADMIN;
            assert_eq!(
                consistency == Consistency::HigherConsistency,
                strict,
                "{m} {path}"
            );
        }
    }

    #[test]
    fn anything_else_is_denied() {
        for (m, path) in [
            (Method::DELETE, "/v1/engines"),
            (Method::PUT, "/v1/namespaces/p1/engines"),
            (Method::GET, "/v1/namespaces/p1/engines/eng_1/sql"),
            (Method::POST, "/v1/namespaces/p1/engines/eng_1/usage"),
            (
                Method::GET,
                "/v1/namespaces/p1/engines/eng_1/jobs/j1/cancel",
            ),
            (Method::GET, "/v1/namespaces/p1/engines/eng_1/other"),
            (Method::GET, "/v1/namespaces//engines"),
            (Method::GET, "/v1/namespaces/a%23b/engines"),
            (Method::GET, "/v1/namespaces/../engines"),
        ] {
            assert!(declare(&m, path).is_none(), "{m} {path}");
        }
    }
}
