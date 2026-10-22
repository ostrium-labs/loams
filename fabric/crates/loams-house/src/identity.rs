//! Who may do what on the House (HS1 Task 20, §49 §14; controller ruling
//! R20.5).
//!
//! Until MT1's identity layer exists, the House authorizes with a static
//! verifier:
//!
//! * **Scopes** ([`Scopes`]): `read` (queries, `SHOW`, `DESCRIBE`, `EXPLAIN`,
//!   `system` tables, `SET`, `USE`, temporary tables), `write` (`INSERT`,
//!   `OPTIMIZE`), `ddl` (`CREATE`, `ALTER`, `DROP`, `TRUNCATE`, `RENAME`,
//!   `UNDROP`, pipes and materialized views) and `admin` (another user's
//!   `KILL QUERY`, `RestoreTable`). A statement needs its scope ([`required`]);
//!   without it the answer is `497`.
//! * **Grants** ([`Grants`]): the `database.table` patterns an identity may
//!   touch at all (`*` matches any name). None configured means every table of
//!   its namespace. A table outside them is not readable, writable or
//!   alterable (`497`), and is hidden from listings.
//! * **Principals** ([`PrincipalMap`]): the engine's link (a House user with
//!   `link = true`) acts for the Loams principal the engine authenticated,
//!   named in [`PRINCIPAL_HEADER`] (and [`EMAIL_HEADER`]). Each principal maps
//!   to a namespace, scopes and grants. **A principal with no mapping has no
//!   House access** (`516`), unless a `--single-node` House was started with
//!   `--dev-unmapped-principals`, when it acts as the local user.
//!
//! MT1's verifier (Loams tokens and API keys, the environment's namespace)
//! replaces [`resolve`]'s static map behind the same [`crate::config::UserMap`]
//! identity: scopes, grants and the principal stay what the rest of the House
//! checks.

use std::fmt;

use crate::config::UserMap;
use crate::errors::{ChError, HouseError};

/// The header naming the Loams principal a link request acts for.
pub const PRINCIPAL_HEADER: &str = "X-Loams-Principal";
/// The header carrying that principal's verified email, when it has one.
pub const EMAIL_HEADER: &str = "X-Loams-Principal-Email";

/// One scope (§49 §14).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// `house:read`.
    Read,
    /// `house:write`.
    Write,
    /// `house:ddl`.
    Ddl,
    /// `house:admin`.
    Admin,
}

impl Scope {
    /// Its name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Ddl => "ddl",
            Self::Admin => "admin",
        }
    }
}

/// A set of scopes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Scopes {
    /// `read`.
    pub read: bool,
    /// `write`.
    pub write: bool,
    /// `ddl`.
    pub ddl: bool,
    /// `admin`.
    pub admin: bool,
}

impl Scopes {
    /// Every scope.
    pub const ALL: Self = Self {
        read: true,
        write: true,
        ddl: true,
        admin: true,
    };
    /// `read` alone.
    pub const READ: Self = Self {
        read: true,
        write: false,
        ddl: false,
        admin: false,
    };

    /// Whether it holds `scope`.
    pub fn has(self, scope: Scope) -> bool {
        match scope {
            Scope::Read => self.read,
            Scope::Write => self.write,
            Scope::Ddl => self.ddl,
            Scope::Admin => self.admin,
        }
    }

    /// Whether it holds nothing that changes data or tables.
    pub fn read_only(self) -> bool {
        !self.write && !self.ddl && !self.admin
    }

    /// Scopes from names (`read`, `write`, `ddl`, `admin`, with or without
    /// `house:`), separated by commas, `+` or spaces.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut out = Self::default();
        for name in text
            .split([',', '+', ' '])
            .map(str::trim)
            .filter(|n| !n.is_empty())
        {
            match name.strip_prefix("house:").unwrap_or(name) {
                "read" => out.read = true,
                "write" => out.write = true,
                "ddl" => out.ddl = true,
                "admin" => out.admin = true,
                other => return Err(format!("unknown scope {other:?} (read, write, ddl, admin)")),
            }
        }
        Ok(out)
    }
}

impl fmt::Display for Scopes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = [Scope::Read, Scope::Write, Scope::Ddl, Scope::Admin]
            .into_iter()
            .filter(|s| self.has(*s))
            .map(Scope::as_str)
            .collect();
        f.write_str(&names.join(","))
    }
}

/// The tables an identity may touch: `database.table` patterns where `*`
/// alone matches any name. Names compare exactly (ClickHouse names are case
/// sensitive).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Grants(Vec<(String, String)>);

impl Grants {
    /// Grants from `db.table` patterns.
    pub fn parse<I, S>(patterns: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Vec::new();
        for pattern in patterns {
            let pattern = pattern.as_ref().trim();
            let Some((database, table)) = pattern.split_once('.') else {
                return Err(format!(
                    "grant {pattern:?}: expected database.table (`*` for any)"
                ));
            };
            if database.is_empty() || table.is_empty() || table.contains('.') {
                return Err(format!(
                    "grant {pattern:?}: expected database.table (`*` for any)"
                ));
            }
            out.push((database.to_string(), table.to_string()));
        }
        Ok(Self(out))
    }

    /// Whether `database.table` is granted.
    pub fn allows(&self, database: &str, table: &str) -> bool {
        // `_house` is the House's own (the query log, HS1 Task 23): no grant
        // reaches it, not even `*.*`.
        if database == crate::querylog::HOUSE_DATABASE {
            return false;
        }
        self.0
            .iter()
            .any(|(d, t)| (d == "*" || d == database) && (t == "*" || t == table))
    }

    /// Whether every table of `database` is granted (what database DDL needs).
    pub fn allows_database(&self, database: &str) -> bool {
        if database == crate::querylog::HOUSE_DATABASE {
            return false;
        }
        self.0
            .iter()
            .any(|(d, t)| (d == "*" || d == database) && t == "*")
    }

    /// Whether some table of `database` may be granted (it is listed).
    pub fn mentions_database(&self, database: &str) -> bool {
        if database == crate::querylog::HOUSE_DATABASE {
            return false;
        }
        self.0.iter().any(|(d, _)| d == "*" || d == database)
    }
}

/// A Loams principal's House identity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PrincipalMap {
    /// The principal (`tailscale:alice@example.com`, `apikey:<id>`,
    /// `user:<sub>`, `session:<id>`, `dev-loopback`) or `email:<address>`.
    pub principal: String,
    /// The namespace its statements run in.
    pub namespace: u64,
    /// Its scopes.
    pub scopes: Scopes,
    /// Its tables; `None`, every table of the namespace.
    pub grants: Option<Grants>,
}

impl PrincipalMap {
    /// Parses `principal=scopes` (the `--principals` list's entries), in
    /// `namespace`.
    pub fn parse(entry: &str, namespace: u64) -> Result<Self, String> {
        let (principal, scopes) = entry
            .split_once('=')
            .ok_or_else(|| format!("principal {entry:?}: expected <principal>=<scopes>"))?;
        let principal = principal.trim();
        if principal.is_empty() {
            return Err(format!("principal {entry:?}: the principal is empty"));
        }
        Ok(Self {
            principal: principal.to_string(),
            namespace,
            scopes: Scopes::parse(scopes)?,
            grants: None,
        })
    }
}

/// `516` for a principal the House does not know.
fn refused(message: impl Into<String>) -> HouseError {
    HouseError::from(ChError::authentication_failed(message.into()))
}

/// The header's value, when present and not empty.
fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

/// The identity a request acts as. `user` signed in; a link user acts for the
/// principal its request names, mapped by `principals`, or by `unmapped` when
/// the House allows that (`--single-node` dev mode). A non-link user may not
/// name a principal.
pub fn resolve(
    user: &UserMap,
    headers: &[(String, String)],
    principals: &[PrincipalMap],
    unmapped: Option<&UserMap>,
) -> Result<UserMap, HouseError> {
    let named = header(headers, PRINCIPAL_HEADER);
    if !user.link {
        if named.is_some() || header(headers, EMAIL_HEADER).is_some() {
            return Err(refused(format!(
                "{}: Authentication failed: only the engine's link may act for a Loams \
                 principal.",
                user.user
            )));
        }
        return Ok(user.clone());
    }
    let Some(principal) = named else {
        return Err(refused(format!(
            "{}: Authentication failed: the engine's link must name the principal it acts for.",
            user.user
        )));
    };
    let email = header(headers, EMAIL_HEADER).map(|e| format!("email:{e}"));
    let found = principals
        .iter()
        .find(|m| m.principal == principal || email.as_deref().is_some_and(|e| m.principal == e));
    match (found, unmapped) {
        (Some(map), _) => Ok(UserMap {
            user: principal.to_string(),
            password_sha256: String::new(),
            namespace: map.namespace,
            // Scopes refuse what a principal may not do (`497`); `readonly`
            // stays ClickHouse's rule for read-only users and GET (`164`).
            readonly: false,
            scopes: map.scopes,
            grants: map.grants.clone(),
            link: false,
            principal: Some(principal.to_string()),
        }),
        (None, Some(local)) => {
            let mut acting = local.clone();
            acting.link = false;
            acting.principal = Some(principal.to_string());
            Ok(acting)
        }
        (None, None) => Err(refused(format!(
            "{principal}: Authentication failed: this principal has no House access (map it \
             with --principals or [[house.principals]])."
        ))),
    }
}

/// `497` for a scope the identity lacks.
pub fn lacks(user: &UserMap, scope: Scope, what: &str) -> HouseError {
    HouseError::from(ChError::access_denied(format!(
        "{}: Not enough privileges. To execute {what} the House scope `{}` is required \
         (granted: {}).",
        user.user,
        scope.as_str(),
        if user.scopes == Scopes::default() {
            "none".to_string()
        } else {
            user.scopes.to_string()
        }
    )))
}

/// `497` for a table outside the identity's grants.
pub fn not_granted(user: &UserMap, database: &str, table: &str) -> HouseError {
    HouseError::from(ChError::access_denied(format!(
        "{}: Not enough privileges on {database}.{table}: it is not among this identity's \
         tables.",
        user.user
    )))
}

/// Whether `user` may touch `database.table`.
pub fn granted(user: &UserMap, database: &str, table: &str) -> bool {
    database != crate::querylog::HOUSE_DATABASE
        && user
            .grants
            .as_ref()
            .is_none_or(|grants| grants.allows(database, table))
}

/// Whether `user` may run DDL on `database` itself.
pub fn granted_database(user: &UserMap, database: &str) -> bool {
    database != crate::querylog::HOUSE_DATABASE
        && user
            .grants
            .as_ref()
            .is_none_or(|grants| grants.allows_database(database))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect()
    }

    fn link() -> UserMap {
        let mut link = UserMap::dev("engine", "k", 1, false);
        link.link = true;
        link
    }

    #[test]
    fn scopes_and_grants_parse_and_match() {
        let s = Scopes::parse("house:read, write").expect("scopes");
        assert!(s.read && s.write && !s.ddl && !s.admin);
        assert_eq!(s.to_string(), "read,write");
        assert!(Scopes::parse("read,root").is_err());
        assert!(Scopes::READ.read_only() && !Scopes::ALL.read_only());

        let g = Grants::parse(["sales.*", "*.events", "web.hits"]).expect("grants");
        assert!(g.allows("sales", "orders"));
        assert!(g.allows("logs", "events"));
        assert!(g.allows("web", "hits"));
        assert!(!g.allows("web", "users"));
        assert!(g.allows_database("sales") && !g.allows_database("web"));
        assert!(g.mentions_database("web") && g.mentions_database("any"));
        for bad in ["sales", ".t", "a.b.c", "a."] {
            assert!(Grants::parse([bad]).is_err(), "{bad}");
        }
    }

    #[test]
    fn principals_are_mapped_or_refused() {
        let principals = vec![
            PrincipalMap::parse("tailscale:alice@x.com=read,write", 7).expect("alice"),
            PrincipalMap::parse("email:bob@x.com=read", 7).expect("bob"),
        ];
        let headers = pairs(&[(PRINCIPAL_HEADER, "tailscale:alice@x.com")]);
        let alice = resolve(&link(), &headers, &principals, None).expect("alice");
        assert_eq!(alice.user, "tailscale:alice@x.com");
        assert_eq!(alice.namespace, 7);
        assert!(alice.scopes.write && !alice.scopes.ddl);
        assert!(!alice.link);
        assert_eq!(alice.principal.as_deref(), Some("tailscale:alice@x.com"));

        // By verified email.
        let headers = pairs(&[(PRINCIPAL_HEADER, "user:abc"), (EMAIL_HEADER, "bob@x.com")]);
        let bob = resolve(&link(), &headers, &principals, None).expect("bob");
        assert!(!bob.readonly && bob.scopes == Scopes::READ);

        // Unmapped: no access, unless dev mode maps it to the local user.
        let headers = pairs(&[(PRINCIPAL_HEADER, "tailscale:eve@x.com")]);
        let err = resolve(&link(), &headers, &principals, None).expect_err("eve");
        assert_eq!(err.code(), 516);
        let local = UserMap::dev("default", "pw", 1, false);
        let dev = resolve(&link(), &headers, &principals, Some(&local)).expect("dev");
        assert_eq!(dev.user, "default");
        assert_eq!(dev.principal.as_deref(), Some("tailscale:eve@x.com"));

        // A link request must name its principal; nobody else may.
        assert_eq!(
            resolve(&link(), &[], &principals, None)
                .expect_err("none")
                .code(),
            516
        );
        let plain = UserMap::dev("default", "pw", 1, false);
        let headers = pairs(&[(PRINCIPAL_HEADER, "tailscale:alice@x.com")]);
        assert_eq!(
            resolve(&plain, &headers, &principals, None)
                .expect_err("not a link")
                .code(),
            516
        );
        assert_eq!(
            resolve(&plain, &[], &principals, None).expect("itself"),
            plain
        );
    }
}
