//! Who is asking (FL2 Task 2; HS1 Task 20 replaces the development users with
//! Loams identities and the MT1 verifier).
//!
//! Credentials come from, **in this order**, the first that is present:
//! `X-ClickHouse-User`/`X-ClickHouse-Key`, HTTP Basic, the `user`/`password`
//! parameters; with none, the user is `default` with an empty password, as in
//! ClickHouse. An unknown user and a wrong password are the same answer, `516
//! AUTHENTICATION_FAILED`, with ClickHouse's own words, so neither leaks which.

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

/// The request's credentials, from the first source that has them.
pub fn credentials(
    headers: &[(String, String)],
    params: &[(String, String)],
) -> Result<Credentials, HouseError> {
    if let Some(user) = header(headers, "X-ClickHouse-User") {
        return Ok(Credentials {
            user: user.to_string(),
            password: header(headers, "X-ClickHouse-Key")
                .unwrap_or("")
                .to_string(),
            source: Source::Headers,
        });
    }
    if let Some(auth) = header(headers, "Authorization") {
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
            user: user.to_string(),
            password: password.to_string(),
            source: Source::Basic,
        });
    }
    if let Some(user) = param(params, "user") {
        return Ok(Credentials {
            user: user.to_string(),
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
    users
        .iter()
        .find(|u| u.user == credentials.user && u.verifies(credentials.password()))
        .ok_or_else(|| {
            failed(
                &credentials.user,
                "password is incorrect, or there is no user with such name",
            )
        })
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
    fn first_source_wins() {
        let headers = pairs(&[
            ("x-clickhouse-user", "h"),
            ("X-ClickHouse-Key", "hk"),
            ("Authorization", "Basic Yjpiaw=="),
        ]);
        let params = pairs(&[("user", "p"), ("password", "pk")]);
        let c = credentials(&headers, &params).expect("credentials");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("h", "hk", Source::Headers)
        );
        let c = credentials(&headers[2..], &params).expect("credentials");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("b", "bk", Source::Basic)
        );
        let c = credentials(&[], &params).expect("credentials");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("p", "pk", Source::Params)
        );
        let c = credentials(&[], &[]).expect("credentials");
        assert_eq!(
            (c.user.as_str(), c.password(), c.source),
            ("default", "", Source::Default)
        );
        assert!(!format!("{c:?}").contains("pk"));
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
