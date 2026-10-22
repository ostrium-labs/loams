//! Commits the server makes for a workspace (plan GT1 Task 27, ruling
//! T27.1): `CommitWorkspace` commits a workspace's index for an agent that
//! does not mount it, so its work can be pushed (`PushWorkspace`) or landed
//! (`Land`, Task 90, D1026).
//!
//! Written from `gitformat-commit`'s documented layout (git's `commit`
//! object: `tree`, `parent` lines, `author`, `committer`, a blank line and
//! the message), never git's source.

use bytes::Bytes;

use crate::ids::ObjectId;
use crate::odb::OdbError;

/// The committer of every commit the server makes for a workspace: the
/// author is whoever asked.
pub const SERVER_COMMITTER: Ident = Ident {
    name: std::borrow::Cow::Borrowed("Loams"),
    email: std::borrow::Cow::Borrowed("loams@loams.invalid"),
};

/// A commit's author or committer: a name and an email, without the time.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ident {
    /// The name, without `<`, `>` or line breaks.
    pub name: std::borrow::Cow<'static, str>,
    /// The email, without `<`, `>`, spaces or line breaks.
    pub email: std::borrow::Cow<'static, str>,
}

impl Ident {
    /// An identity from a principal's display name and email, falling back
    /// to its stable subject (`<subject>@loams.invalid` without an email).
    /// Characters git's signature line cannot hold are dropped (`<`, `>`
    /// and control characters; spaces too in the email).
    pub fn of_principal(subject: &str, display_name: &str, email: Option<&str>) -> Self {
        let clean = |raw: &str, spaces: bool| -> String {
            raw.chars()
                .filter(|c| !matches!(c, '<' | '>') && !c.is_control() && (spaces || *c != ' '))
                .collect::<String>()
                .trim()
                .to_owned()
        };
        let mut name = clean(display_name, true);
        if name.is_empty() {
            name = clean(subject, true);
        }
        if name.is_empty() {
            name = "loams user".to_owned();
        }
        let mut address = email.map(|e| clean(e, false)).unwrap_or_default();
        if address.is_empty() {
            let local = clean(subject, false);
            let local = if local.is_empty() {
                "user".into()
            } else {
                local
            };
            address = format!("{local}@loams.invalid");
        }
        Self {
            name: name.into(),
            email: address.into(),
        }
    }

    /// `Name <email>`, as a signature line holds it.
    pub fn line(&self) -> String {
        format!("{} <{}>", self.name, self.email)
    }
}

/// A workspace commit: `tree`, at most one `parent` (none for the first
/// commit of an unborn workspace), `author` and `committer` at
/// `unix_seconds` in `+0000`, and `message` (a final newline added when
/// missing). Answers its id and its body (without git's `commit <len>\0`
/// header).
pub fn workspace_commit(
    tree: &ObjectId,
    parent: Option<&ObjectId>,
    author: &Ident,
    committer: &Ident,
    unix_seconds: i64,
    message: &str,
) -> Result<(ObjectId, Bytes), OdbError> {
    let mut body = format!("tree {tree}\n");
    if let Some(parent) = parent {
        body.push_str(&format!("parent {parent}\n"));
    }
    body.push_str(&format!("author {} {unix_seconds} +0000\n", author.line()));
    body.push_str(&format!(
        "committer {} {unix_seconds} +0000\n",
        committer.line()
    ));
    body.push('\n');
    body.push_str(message);
    if !message.ends_with('\n') {
        body.push('\n');
    }
    let oid = gix_object::compute_hash(
        gix_hash::Kind::Sha1,
        gix_object::Kind::Commit,
        body.as_bytes(),
    )
    .map_err(|err| OdbError::CorruptObject {
        oid: *tree,
        reason: format!("hashing a commit: {err}"),
    })?;
    Ok((oid, Bytes::from(body)))
}

/// Whether commit body `stored` is the commit [`workspace_commit`] makes
/// from these fields at some time: the same tree, parent, author and
/// committer identities and message. A retried `CommitWorkspace` answers
/// the commit its first attempt made when this holds.
pub fn same_workspace_commit(
    stored: &[u8],
    tree: &ObjectId,
    parent: Option<&ObjectId>,
    author: &Ident,
    message: &str,
) -> bool {
    let Ok(commit) = gix_object::CommitRef::from_bytes(stored, gix_hash::Kind::Sha1) else {
        return false;
    };
    let parents: Vec<ObjectId> = commit.parents().collect();
    let ident = |raw: &[u8]| {
        let sig = crate::history::log::parse_signature(raw);
        (sig.name, sig.email)
    };
    let mut wanted = message.to_owned();
    if !wanted.ends_with('\n') {
        wanted.push('\n');
    }
    commit.tree() == *tree
        && parents.as_slice() == parent.map(std::slice::from_ref).unwrap_or_default()
        && ident(commit.author.as_ref())
            == (
                author.name.as_bytes().to_vec(),
                author.email.as_bytes().to_vec(),
            )
        && ident(commit.committer.as_ref())
            == (
                SERVER_COMMITTER.name.as_bytes().to_vec(),
                SERVER_COMMITTER.email.as_bytes().to_vec(),
            )
        && AsRef::<[u8]>::as_ref(commit.message) == wanted.as_bytes()
}
