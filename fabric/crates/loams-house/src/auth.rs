//! Who is asking (FL2 Task 2; HS1 Task 20 replaces the development users with
//! Loams identities and the MT1 verifier).
//!
//! Credentials come from exactly one of: `X-ClickHouse-User`/`X-ClickHouse-Key`,
//! HTTP Basic, or the `user`/`password` parameters. A request that mixes them is
//! refused with `516`, as ClickHouse refuses it (Task 3 review M8; this replaces
//! FL2 Task 2's "first one wins"). With none, the user is `default` with an empty
//! password. An empty `X-ClickHouse-User` counts as absent. An unknown user and a
//! wrong password are the same answer, `516 AUTHENTICATION_FAILED`, with ClickHouse's
//! own words and the same work (a digest is compared either way), so neither the
//! text nor the timing says which.

use std::fmt;

use base64::Engine as _;

use crate::config::UserMap;
use crate::errors::{ChError, HouseError};

/// Where the credentials came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// `X-ClickHouse-User` / `X-ClickHouse-Key`.
    Headers,
    /// `Authorization: Basic …`.
    Basic,
    /// `user` / `password` parameters.
    Params,
    /// None given: `default` with no password.
    Default,
}

/// A user name and password. The password never prints.
#[derive(Clone, PartialEq, Eq)]
pub struct Credentials {
    /// The user name.
    pub user: String,
    password: String,
    /// Where they came from.
    pub source: Source,
}

impl fmt::Debug for Credentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Credentials")
            .field("user", &self.user)
            .field("password", &"[redacted]")
            .field("source", &self.source)
            .finish()
    }
}

impl Credentials {
    /// The password, for the one comparison that needs it.
    pub fn password(&self) -> &str {
        &self.password
    }
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

fn param<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// The request's credentials, from the one source that has them.
pub fn credentials(
    headers: &[(String, String)],
    params: &[(String, String)],
) -> Result<Credentials, HouseError> {
    let header_user = header(headers, "X-ClickHouse-User").filter(|u| !u.is_empty());
    let header_key = header(headers, "X-ClickHouse-Key");
    let by_headers = header_user.is_some() || header_key.is_some();
    let basic = header(headers, "Authorization");
    let by_params = param(params, "user").is_some() || param(params, "password").is_some();

    if by_headers && (basic.is_some() || by_params) {
        return Err(invalid(
            "it is not allowed to use X-ClickHouse HTTP headers and other authentication \
             methods simultaneously",
        ));
    }
    if basic.is_some() && by_params {
        return Err(invalid(
            "it is not allowed to use Authorization HTTP header and authentication via \
             parameters simultaneously",
        ));
    }
    if by_headers {
        return Ok(Credentials {
            user: header_user.unwrap_or("default").to_string(),
            password: header_key.unwrap_or("").to_string(),
            source: Source::Headers,
        });
    }
    if let Some(auth) = basic {
        let encoded = auth
            .strip_prefix("Basic ")
            .or_else(|| auth.strip_prefix("basic "))
            .ok_or_else(|| failed("", "the Authorization header is not Basic"))?;
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded.trim())
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .ok_or_else(|| failed("", "the Basic credentials are not base64 text"))?;
        let (user, password) = decoded.split_once(':').unwrap_or((decoded.as_str(), ""));
        return Ok(Credentials {
            user: if user.is_empty() { "default" } else { user }.to_string(),
            password: password.to_string(),
            source: Source::Basic,
        });
    }
    if by_params {
        return Ok(Credentials {
            user: param(params, "user")
                .filter(|u| !u.is_empty())
                .unwrap_or("default")
                .to_string(),
            password: param(params, "password").unwrap_or("").to_string(),
            source: Source::Params,
        });
    }
    Ok(Credentials {
        user: "default".to_string(),
        password: String::new(),
        source: Source::Default,
    })
}

/// The user these credentials name, if the password is theirs.
pub fn authenticate<'a>(
    users: &'a [UserMap],
    credentials: &Credentials,
) -> Result<&'a UserMap, HouseError> {
    let found = users.iter().find(|u| u.user == credentials.user);
    // An unknown user is compared against a dummy, so it costs what a known one does.
    let dummy = UserMap::dev("", "\u{0}loams-no-such-user", 0, true);
    let verified = found.unwrap_or(&dummy).verifies(credentials.password());
    found.filter(|_| verified).ok_or_else(|| {
        failed(
            &credentials.user,
            "password is incorrect, or there is no user with such name",
        )
    })
}

/// ClickHouse's `516` for mixed credential sources.
fn invalid(why: &str) -> HouseError {
    HouseError::from(ChError::authentication_failed(format!(
        "Invalid authentication: {why}"
    )))
}

/// ClickHouse's `516` text: `<user>: Authentication failed: <why>.`
fn failed(user: &str, why: &str) -> HouseError {
    HouseError::from(ChError::authentication_failed(format!(
        "{user}: Authentication failed: {why}."
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    #[test]
    fn each_source_alone() {
        let c = credentials(
            &pairs(&[("x-clickhouse-user", "h"), ("X-ClickHouse-Key", "hk")]),
            &[],
        )
        .expect("headers");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("h", "hk", Source::Headers)
        );
        let c = credentials(&pairs(&[("Authorization", "Basic Yjpiaw==")]), &[]).expect("basic");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("b", "bk", Source::Basic)
        );
        let c = credentials(&[], &pairs(&[("user", "p"), ("password", "pk")])).expect("params");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("p", "pk", Source::Params)
        );
        let c = credentials(&[], &[]).expect("none");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("default", "", Source::Default)
        );
        assert!(!format!("{c:?}").contains("pk"));
        // An empty X-ClickHouse-User is absent: Basic alone is then fine.
        let c = credentials(
            &pairs(&[
                ("X-ClickHouse-User", ""),
                ("Authorization", "Basic Yjpiaw=="),
            ]),
            &[],
        )
        .expect("empty header user is absent");
        assert_eq!(c.source, Source::Basic);
    }

    #[test]
    fn mixed_sources_are_516() {
        let headers = pairs(&[("X-ClickHouse-User", "h")]);
        let basic = pairs(&[("Authorization", "Basic Yjpiaw==")]);
        let both = pairs(&[
            ("X-ClickHouse-Key", "k"),
            ("Authorization", "Basic Yjpiaw=="),
        ]);
        let params = pairs(&[("user", "p")]);
        for (h, p) in [(&headers, &params), (&basic, &params), (&both, &Vec::new())] {
            let err = credentials(h, p).expect_err("mixed");
            assert_eq!(err.code(), 516);
            assert!(
                err.message().starts_with("Invalid authentication:"),
                "{err}"
            );
        }
    }

    #[test]
    fn unknown_user_and_wrong_password_are_one_answer() {
        let users = vec![UserMap::dev("alice", "secret", 1, false)];
        for (user, password) in [("alice", "nope"), ("bob", "secret")] {
            let c = credentials(&[], &pairs(&[("user", user), ("password", password)]))
                .expect("credentials");
            let err = authenticate(&users, &c).expect_err(user);
            assert_eq!(err.code(), 516);
            assert_eq!(
                err.message(),
                format!(
                    "{user}: Authentication failed: password is incorrect, or there is no \
                     user with such name."
                )
            );
        }
        let ok = credentials(&[], &pairs(&[("user", "alice"), ("password", "secret")]))
            .expect("credentials");
        assert_eq!(authenticate(&users, &ok).expect("alice").namespace, 1);
    }
}
