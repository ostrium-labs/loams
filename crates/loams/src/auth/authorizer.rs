//! What an admitted principal may do in the org (API2 Task 7): every
//! `loams.admin.v1` RPC asks an [`Authorizer`] for the caller's standing
//! before it reads or changes anything.
//!
//! [`OrgRoleAuthorizer`] is the implementation until MT1 replaces it with
//! OpenFGA: the org role (`owner` > `admin` > `member`) of the caller, from
//! the principal store ([`super::directory`]). How a principal maps to a
//! role:
//!
//! - a console session: its person's role;
//! - an API key `CreateKey` made: the role stored with it (its creator's for
//!   a key acting as its creator, `member` for a service account's);
//! - an API key from `LOAMS_API_KEYS_FILE` and the developer on loopback
//!   (`LOAMS_AUTH_MODE=off`): `owner`, since the operator made them;
//! - an OIDC or tailnet user: the role of the member with their email; a
//!   person with a pending invitation joins with the invitation's role on
//!   their first admin call; anyone else is not in the org.
//!
//! Before first-run setup there is no org, and every check fails with
//! [`Denied::SetupRequired`].

use std::sync::Arc;

use async_trait::async_trait;
use loams_proto::loams::admin::v1 as pb;

use super::directory::{Directory, MemberRec, Role, Unavailable, new_id, now_ms, timestamp};
use super::{Principal, PrincipalKind};

/// Who is acting, as the audit log names them, and their role.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Actor {
    pub kind: pb::SubjectKind,
    pub id: String,
    pub name: String,
    pub role: Role,
    /// The member's user id when the actor is a person in the org.
    pub user: Option<String>,
}

impl Actor {
    /// The actor as a `SubjectRef`.
    pub fn subject(&self) -> pb::SubjectRef {
        pb::SubjectRef {
            kind: self.kind.into(),
            id: self.id.clone(),
            name: self.name.clone(),
            ..Default::default()
        }
    }
}

/// Why an [`Authorizer`] refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denied {
    /// The org does not exist yet: `AuthService/CompleteSetup` first.
    SetupRequired,
    /// The caller's role is below `need` (`have` is `None` for someone
    /// outside the org).
    RoleRequired { need: Role, have: Option<Role> },
    /// The principal store could not be read.
    Unavailable(String),
}

impl From<Unavailable> for Denied {
    fn from(err: Unavailable) -> Self {
        Self::Unavailable(err.0)
    }
}

/// Decides whether `principal` holds at least `need` in the org. MT1's
/// OpenFGA authorizer replaces [`OrgRoleAuthorizer`] behind this trait.
#[async_trait]
pub trait Authorizer: Send + Sync + 'static {
    async fn authorize(&self, principal: &Principal, need: Role) -> Result<Actor, Denied>;
}

/// Org roles from the principal store.
#[derive(Debug, Clone)]
pub struct OrgRoleAuthorizer {
    directory: Arc<Directory>,
}

impl OrgRoleAuthorizer {
    pub fn new(directory: Arc<Directory>) -> Self {
        Self { directory }
    }

    fn member_actor(member: &MemberRec) -> Actor {
        Actor {
            kind: pb::SubjectKind::SUBJECT_KIND_USER,
            id: member.user.id.clone(),
            name: member.user.name.clone(),
            role: member.role,
            user: Some(member.user.id.clone()),
        }
    }

    /// The operator's own credentials: the key file and the developer on
    /// loopback.
    fn operator(principal: &Principal, kind: pb::SubjectKind) -> Actor {
        Actor {
            kind,
            id: principal.subject.clone(),
            name: principal.display_name.clone(),
            role: Role::Owner,
            user: None,
        }
    }

    /// An externally authenticated person: their membership, or the
    /// invitation they accept by calling.
    async fn person(&self, principal: &Principal) -> Result<Option<Actor>, Denied> {
        let Some(email) = principal.email.as_deref().filter(|e| !e.is_empty()) else {
            return Ok(None);
        };
        let doc = self.directory.read().await?;
        if let Some(member) = doc.member_by_email(email) {
            return Ok(Some(Self::member_actor(member)));
        }
        let now = now_ms();
        let invited = doc.invitations.iter().any(|i| {
            i.email.eq_ignore_ascii_case(email)
                && i.expires_at
                    .as_option()
                    .is_none_or(|t| super::directory::ms_of(t) > now)
        });
        if !invited {
            return Ok(None);
        }
        let name = principal.display_name.clone();
        let joined = self
            .directory
            .update(|doc| {
                if let Some(member) = doc.member_by_email(email) {
                    return Ok(Some(member.clone()));
                }
                let Some(at) = doc.invitations.iter().position(|i| {
                    i.email.eq_ignore_ascii_case(email)
                        && i.expires_at
                            .as_option()
                            .is_none_or(|t| super::directory::ms_of(t) > now)
                }) else {
                    return Ok(None);
                };
                let invitation = doc.invitations.remove(at);
                let role = invitation
                    .role
                    .as_known()
                    .and_then(Role::from_proto)
                    .unwrap_or(Role::Member);
                let member = MemberRec {
                    user: pb::User {
                        id: new_id("usr"),
                        name: if name.is_empty() {
                            email.to_owned()
                        } else {
                            name.clone()
                        },
                        email: email.to_owned(),
                        created_at: timestamp(now).into(),
                        last_seen_at: timestamp(now).into(),
                        ..Default::default()
                    },
                    role,
                    joined_at_ms: now,
                    password: String::new(),
                };
                for team in &invitation.teams {
                    if let Some(rec) = doc
                        .teams
                        .iter_mut()
                        .find(|t| t.team.slug == *team || t.team.id == *team)
                    {
                        rec.members.push(member.user.id.clone());
                    }
                }
                doc.members.push(member.clone());
                doc.record(pb::AuditEvent {
                    actor: pb::SubjectRef {
                        kind: pb::SubjectKind::SUBJECT_KIND_USER.into(),
                        id: member.user.id.clone(),
                        name: member.user.name.clone(),
                        ..Default::default()
                    }
                    .into(),
                    action: "invitation.accept".to_owned(),
                    target: pb::ResourceRef {
                        kind: "invitation".to_owned(),
                        id: invitation.id.clone(),
                        name: invitation.email.clone(),
                        ..Default::default()
                    }
                    .into(),
                    outcome: pb::AuditOutcome::AUDIT_OUTCOME_SUCCESS.into(),
                    ..Default::default()
                });
                Ok::<_, Denied>(Some(member))
            })
            .await?;
        Ok(joined.as_ref().map(Self::member_actor))
    }
}

#[async_trait]
impl Authorizer for OrgRoleAuthorizer {
    async fn authorize(&self, principal: &Principal, need: Role) -> Result<Actor, Denied> {
        let doc = self.directory.read().await?;
        if doc.org.is_none() {
            return Err(Denied::SetupRequired);
        }
        let actor = match principal.kind {
            PrincipalKind::DevLoopback => Some(Self::operator(
                principal,
                pb::SubjectKind::SUBJECT_KIND_USER,
            )),
            PrincipalKind::Session => doc.member(&principal.subject).map(Self::member_actor),
            PrincipalKind::ApiKey => {
                let id = principal
                    .subject
                    .strip_prefix("apikey:")
                    .unwrap_or(&principal.subject);
                match doc.key(id) {
                    Some(key) => {
                        let subject = key.key.principal.as_option().cloned().unwrap_or_default();
                        let kind = subject
                            .kind
                            .as_known()
                            .unwrap_or(pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT);
                        // A key acting as a person has that person's live role at most: none
                        // once they have left the org (ruling T5.R1), and never more than the
                        // role it was made with.
                        let role = if kind == pb::SubjectKind::SUBJECT_KIND_USER {
                            doc.member(&subject.id).map(|m| m.role.min(key.role))
                        } else {
                            Some(key.role)
                        };
                        role.map(|role| Actor {
                            kind,
                            user: (kind == pb::SubjectKind::SUBJECT_KIND_USER)
                                .then(|| subject.id.clone()),
                            id: subject.id,
                            name: subject.name,
                            role,
                        })
                    }
                    None => Some(Self::operator(
                        principal,
                        pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT,
                    )),
                }
            }
            PrincipalKind::User | PrincipalKind::Tailscale => self.person(principal).await?,
        };
        match actor {
            Some(actor) if actor.role >= need => Ok(actor),
            Some(actor) => Err(Denied::RoleRequired {
                need,
                have: Some(actor.role),
            }),
            None => Err(Denied::RoleRequired { need, have: None }),
        }
    }
}
