//! `loams.admin.v1` on the main port (API2 Task 7, server half): the org,
//! projects and environments, agents and service accounts, API keys and the
//! audit log, over the principal store ([`crate::auth::directory`]).
//!
//! **Authorization.** Every RPC asks the [`Authorizer`] for the caller's
//! standing first: reads need `member`, every mutation `admin`, and removing
//! an owner or inviting one `owner` ([`OrgRoleAuthorizer`] reads the org
//! role from the store until MT1's OpenFGA replaces it). A refusal is
//! `permission_denied` with the reason `role_required` and the role in
//! `metadata["role"]`; before first-run setup it is `failed_precondition`,
//! reason `setup_required`.
//!
//! **What is real.** Every record is kept, so a create shows up in the next
//! list (unlike the console mock, ruling 6/7.10). `CreateKey` writes the
//! key's argon2id hash, which the auth layer then admits as a bearer, and
//! answers the secret once per `idempotency_key` (a replay is
//! `secret_already_issued`). Every mutation appends one audit event, which
//! `ListAuditEvents` lists and `WatchAuditEvents` streams.
//!
//! **What waits for other plans.** Agent tokens are minted by MT1's issuer,
//! so `ListAgentTokens` answers none and `RevokeToken` records the `jti` on a
//! revocation list for the issuer to honour. Environment statistics and
//! `GetEnvironmentActivity` answer zeros and an empty series until metering
//! reports per-environment counters.
//!
//! [`OrgRoleAuthorizer`]: crate::auth::authorizer::OrgRoleAuthorizer

#![allow(refining_impl_trait)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use connectrpc::{
    ConnectError, ErrorCode, RequestContext, Response, ServiceRequest, ServiceResult, ServiceStream,
};
use loams_proto::loams::admin::v1::__buffa::oneof::watch_audit_events_response::Event as WatchEvent;
use loams_proto::loams::admin::v1::{
    self as pb, AddTeamMemberRequest, AddTeamMemberResponse, AgentService, AgentServiceExt,
    AuditService, AuditServiceExt, CreateAgentRequest, CreateAgentResponse,
    CreateEnvironmentRequest, CreateEnvironmentResponse, CreateInvitationRequest,
    CreateInvitationResponse, CreateKeyRequest, CreateKeyResponse, CreateProjectRequest,
    CreateProjectResponse, CreateServiceAccountRequest, CreateServiceAccountResponse,
    CreateTeamRequest, CreateTeamResponse, CreateTrustPolicyRequest, CreateTrustPolicyResponse,
    GetAgentRequest, GetAgentResponse, GetEnvironmentActivityRequest,
    GetEnvironmentActivityResponse, GetEnvironmentRequest, GetEnvironmentResponse, GetOrgRequest,
    GetOrgResponse, GetProjectRequest, GetProjectResponse, GetTeamRequest, GetTeamResponse,
    GrantAccessRequest, GrantAccessResponse, KeyService, KeyServiceExt, ListAccessRequest,
    ListAccessResponse, ListAgentTokensRequest, ListAgentTokensResponse, ListAgentsRequest,
    ListAgentsResponse, ListAuditEventsRequest, ListAuditEventsResponse, ListEnvironmentsRequest,
    ListEnvironmentsResponse, ListInvitationsRequest, ListInvitationsResponse, ListKeysRequest,
    ListKeysResponse, ListMembersRequest, ListMembersResponse, ListProjectsRequest,
    ListProjectsResponse, ListServiceAccountsRequest, ListServiceAccountsResponse,
    ListTeamsRequest, ListTeamsResponse, ListTrustPoliciesRequest, ListTrustPoliciesResponse,
    OrgService, OrgServiceExt, ProjectService, ProjectServiceExt, RemoveMemberRequest,
    RemoveMemberResponse, ResumeAgentRequest, ResumeAgentResponse, RevokeKeyRequest,
    RevokeKeyResponse, RevokeTokenRequest, RevokeTokenResponse, SuspendAgentRequest,
    SuspendAgentResponse, UpdateAgentRequest, UpdateAgentResponse, UpdateOrgRequest,
    UpdateOrgResponse, WatchAuditEventsRequest, WatchAuditEventsResponse,
};
use tokio::sync::broadcast::error::RecvError;

use super::connect::refuse;
use super::connect_console::{MAX_SLUG, event, invalid, slugify, unavailable};
use crate::auth::authorizer::{Actor, Denied};
use crate::auth::authz::{self, Standing};
use crate::auth::directory::{AuditRec, Doc, KeyRec, Role, new_id, now_ms, timestamp};
use crate::auth::{Principal, Principals, api_key, reason};

/// The most a list answers in one page, and what `page_size` 0 gets.
const MAX_PAGE: usize = 1_000;
const DEFAULT_PAGE: usize = 100;
/// The watch's snapshot when `snapshot_size` is 0, and the most it takes.
const DEFAULT_SNAPSHOT: usize = 50;
const MAX_SNAPSHOT: usize = 1_000;
/// How often `WatchAuditEvents` sends a heartbeat (Task 7).
pub const HEARTBEAT: Duration = Duration::from_secs(15);
/// How long an invitation stays open.
const INVITATION_TTL_MS: i64 = 7 * 24 * 3600 * 1000;
/// An agent token's lifetime cap when `CreateAgent` names none.
const DEFAULT_MAX_TOKEN_TTL_S: i32 = 3600;

/// `not_found` with the registered metadata.
fn not_found(kind: &str, name: &str) -> ConnectError {
    refuse(
        ErrorCode::NotFound,
        "not_found",
        format!("no {kind} `{name}`"),
        &[("kind", kind.to_owned()), ("name", name.to_owned())],
    )
}

fn already_exists(kind: &str, name: &str) -> ConnectError {
    refuse(
        ErrorCode::AlreadyExists,
        "already_exists",
        format!("{kind} `{name}` already exists"),
        &[],
    )
}

/// An [`Authorizer`](crate::auth::authorizer::Authorizer) refusal as an RPC
/// error.
pub(crate) fn denied(denied: Denied) -> ConnectError {
    match denied {
        Denied::SetupRequired => refuse(
            ErrorCode::FailedPrecondition,
            "setup_required",
            "this engine has no org yet: run first-run setup (AuthService/CompleteSetup)",
            &[],
        ),
        Denied::RoleRequired { need, have } => refuse(
            ErrorCode::PermissionDenied,
            "role_required",
            match have {
                Some(have) => format!(
                    "this needs the org role {} or higher; the caller is {}",
                    need.name(),
                    have.name()
                ),
                None => format!(
                    "this needs the org role {} or higher; the caller is not in the org",
                    need.name()
                ),
            },
            &[("role", need.name().to_owned())],
        ),
        Denied::Unavailable(message) => unavailable(&message),
    }
}

/// `requested` if given (and valid), else the slug of `name`.
fn slug_for(requested: &str, name: &str) -> Result<String, ConnectError> {
    if requested.is_empty() {
        let slug = slugify(name);
        if slug.is_empty() {
            return Err(invalid("name", "needs a letter or a digit"));
        }
        return Ok(slug);
    }
    let ok = requested.len() <= MAX_SLUG
        && requested
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !requested.starts_with('-')
        && !requested.ends_with('-');
    if ok {
        Ok(requested.to_owned())
    } else {
        Err(invalid(
            "slug",
            "lower-case letters, digits and inner hyphens, at most 63",
        ))
    }
}

/// One AIP-158 page of `items`; the token is the offset of the next page.
fn page<T>(
    items: Vec<T>,
    page_size: i32,
    page_token: &str,
) -> Result<(Vec<T>, String), ConnectError> {
    let size = match usize::try_from(page_size) {
        Ok(0) => DEFAULT_PAGE,
        Ok(n) => n.min(MAX_PAGE),
        Err(_) => return Err(invalid("page_size", "must not be negative")),
    };
    let start = if page_token.is_empty() {
        0
    } else {
        page_token
            .strip_prefix('o')
            .and_then(|n| n.parse::<usize>().ok())
            .filter(|n| *n <= items.len())
            .ok_or_else(|| invalid("page_token", "not a token this list issued"))?
    };
    let end = (start + size).min(items.len());
    let next = if end < items.len() {
        format!("o{end}")
    } else {
        String::new()
    };
    Ok((
        items.into_iter().skip(start).take(end - start).collect(),
        next,
    ))
}

/// The field mask's paths, in either spelling.
fn mask_paths(
    mask: &buffa::MessageField<
        loams_proto::google::protobuf::FieldMask,
        buffa::Inline<loams_proto::google::protobuf::FieldMask>,
    >,
) -> Vec<String> {
    mask.as_option()
        .map(|m| m.paths.clone())
        .unwrap_or_default()
}

/// The console API's admin services.
struct Admin {
    principals: Principals,
    heartbeat: Duration,
}

impl Admin {
    /// The caller, if they hold at least `need` and the RPC's declared `can_*` relation on the
    /// org (IA2 Task 5, D1611). Deny by default: an RPC [`super::admin_rpcs`] does not declare
    /// is refused before anything is read. Both answers must allow, so the model never widens
    /// what the org roles give.
    async fn actor(&self, ctx: &RequestContext, need: Role) -> Result<Actor, ConnectError> {
        let procedure = ctx.path().unwrap_or_default();
        let Some(relation) = super::admin_rpcs::declared(procedure) else {
            return Err(refuse(
                ErrorCode::PermissionDenied,
                "rpc_not_declared",
                "no authorization is declared for this RPC",
                &[],
            ));
        };
        let principal = ctx.extensions().get::<Principal>().ok_or_else(|| {
            refuse(
                ErrorCode::Unauthenticated,
                reason::TOKEN_MISSING,
                "this RPC needs a credential",
                &[],
            )
        })?;
        let actor = self
            .principals
            .authorizer
            .authorize(principal, need)
            .await
            .map_err(denied)?;
        let subject = match authz::standing(&self.principals, principal)
            .await
            .map_err(denied)?
        {
            Standing::Operator => return Ok(actor),
            Standing::Outside => {
                return Err(denied(Denied::RoleRequired { need, have: None }));
            }
            Standing::Subject(subject) => subject,
        };
        let doc = self.doc().await?;
        let Some(org) = doc.org.as_ref() else {
            return Err(denied(Denied::SetupRequired));
        };
        let mut check = loams_authz::CheckRequest::new(
            subject,
            relation,
            loams_authz::ObjectRef::new("org", &org.id),
        );
        if relation == "can_admin" {
            // Changes ask the freshest answer (D1611).
            check = check.consistency(loams_authz::Consistency::HigherConsistency);
        }
        let decision = self.principals.authz.check(check).await.map_err(|err| {
            tracing::warn!(error = %err, procedure, "the authorizer could not answer an admin RPC");
            unavailable("authorization is unavailable; retry")
        })?;
        if decision.allowed {
            Ok(actor)
        } else {
            Err(denied(Denied::RoleRequired {
                need,
                have: Some(actor.role),
            }))
        }
    }

    async fn doc(&self) -> Result<Arc<Doc>, ConnectError> {
        Ok(self.principals.directory.read().await?)
    }

    async fn update<T>(
        &self,
        change: impl FnMut(&mut Doc) -> Result<T, ConnectError>,
    ) -> Result<T, ConnectError> {
        self.principals.directory.update(change).await
    }
}

// ----- Views: the stored records with their computed fields -----

fn member_view(doc: &Doc, member: &crate::auth::directory::MemberRec) -> pb::Member {
    pb::Member {
        user: member.user.clone().into(),
        role: member.role.to_proto().into(),
        teams: doc
            .teams
            .iter()
            .filter(|t| t.members.contains(&member.user.id))
            .map(|t| t.team.slug.clone())
            .collect(),
        joined_at: timestamp(member.joined_at_ms).into(),
        ..Default::default()
    }
}

fn team_view(rec: &crate::auth::directory::TeamRec) -> pb::Team {
    let mut team = rec.team.clone();
    team.member_count = i32::try_from(rec.members.len()).unwrap_or(i32::MAX);
    team
}

fn project_view(doc: &Doc, rec: &crate::auth::directory::ProjectRec) -> pb::Project {
    let mut project = rec.project.clone();
    project.environment_count = i32::try_from(rec.environments.len()).unwrap_or(i32::MAX);
    project.agent_count = i32::try_from(
        doc.agents
            .iter()
            .filter(|a| a.agent.project == project.slug)
            .count(),
    )
    .unwrap_or(i32::MAX);
    project
}

fn service_account_view(doc: &Doc, account: &pb::ServiceAccount) -> pb::ServiceAccount {
    let mut account = account.clone();
    account.key_count = i32::try_from(
        doc.keys
            .iter()
            .filter(|k| {
                !k.revoked
                    && k.key
                        .principal
                        .as_option()
                        .is_some_and(|p| p.id == account.id)
            })
            .count(),
    )
    .unwrap_or(i32::MAX);
    account
}

/// The name of a subject, filled in on the way out.
fn subject_named(doc: &Doc, kind: pb::SubjectKind, id: &str) -> Option<pb::SubjectRef> {
    let name = match kind {
        pb::SubjectKind::SUBJECT_KIND_USER => doc.member(id).map(|m| m.user.name.clone()),
        pb::SubjectKind::SUBJECT_KIND_TEAM => doc.team(id).map(|t| t.team.name.clone()),
        pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT => doc
            .service_accounts
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone()),
        pb::SubjectKind::SUBJECT_KIND_AGENT => doc.agent(id).map(|a| a.agent.name.clone()),
        _ => None,
    }?;
    let id = match kind {
        // A team is named by its id, whichever the request used.
        pb::SubjectKind::SUBJECT_KIND_TEAM => doc.team(id).map(|t| t.team.id.clone())?,
        _ => id.to_owned(),
    };
    Some(pb::SubjectRef {
        kind: kind.into(),
        id,
        name,
        ..Default::default()
    })
}

fn environment<'a>(
    doc: &'a Doc,
    project: &str,
    environment: &str,
) -> Result<&'a pb::Environment, ConnectError> {
    let rec = doc
        .project(project)
        .ok_or_else(|| not_found("project", project))?;
    rec.environments
        .iter()
        .find(|e| e.slug == environment)
        .ok_or_else(|| not_found("environment", &format!("{project}/{environment}")))
}

// ----- OrgService -----

impl OrgService for Admin {
    async fn get_org(
        &self,
        ctx: RequestContext,
        _request: ServiceRequest<'_, GetOrgRequest>,
    ) -> ServiceResult<GetOrgResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        Response::ok(GetOrgResponse {
            org: doc.org.clone().unwrap_or_default().into(),
            ..Default::default()
        })
    }

    async fn update_org(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, UpdateOrgRequest>,
    ) -> ServiceResult<UpdateOrgResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let changes = request.org.as_option().cloned().unwrap_or_default();
        let paths = mask_paths(&request.update_mask);
        let org = self
            .update(|doc| {
                let mut org = doc.org.clone().unwrap_or_default();
                for path in &paths {
                    match path.as_str() {
                        "name" => {
                            if changes.name.trim().is_empty() {
                                return Err(invalid("org.name", "required"));
                            }
                            org.name.clone_from(&changes.name);
                        }
                        "require_two_factor" | "requireTwoFactor" => {
                            org.require_two_factor = changes.require_two_factor;
                        }
                        "allowed_domains" | "allowedDomains" => {
                            org.allowed_domains.clone_from(&changes.allowed_domains);
                        }
                        other => {
                            return Err(invalid(
                                "update_mask",
                                &format!("`{other}` cannot be changed"),
                            ));
                        }
                    }
                }
                if !paths.is_empty() {
                    doc.org = Some(org.clone());
                    doc.record(event(
                        &actor,
                        "org.update",
                        ("org", &org.id, &org.name),
                        "",
                        "",
                    ));
                }
                Ok(org)
            })
            .await?;
        Response::ok(UpdateOrgResponse {
            org: org.into(),
            ..Default::default()
        })
    }

    async fn list_members(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListMembersRequest>,
    ) -> ServiceResult<ListMembersResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let members = doc.members.iter().map(|m| member_view(&doc, m)).collect();
        let (members, next_page_token) = page(members, request.page_size, request.page_token)?;
        Response::ok(ListMembersResponse {
            members,
            next_page_token,
            ..Default::default()
        })
    }

    async fn remove_member(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RemoveMemberRequest>,
    ) -> ServiceResult<RemoveMemberResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let user = request.user.to_owned();
        if user.is_empty() {
            return Err(invalid("user", "required"));
        }
        self.update(|doc| {
            let Some(at) = doc.members.iter().position(|m| m.user.id == user) else {
                return Err(not_found("user", &user));
            };
            let member = doc.members[at].clone();
            if member.role == Role::Owner {
                if actor.role < Role::Owner {
                    return Err(denied(Denied::RoleRequired {
                        need: Role::Owner,
                        have: Some(actor.role),
                    }));
                }
                if doc.members.iter().filter(|m| m.role == Role::Owner).count() == 1 {
                    return Err(refuse(
                        ErrorCode::FailedPrecondition,
                        "failed_precondition",
                        "the org's last owner cannot be removed",
                        &[],
                    ));
                }
            }
            doc.members.remove(at);
            doc.sessions.retain(|s| s.user != user);
            // The keys that act as them go with them (ruling T5.R1); a service account's keys
            // stay, whoever made them.
            let keys: Vec<_> = doc
                .keys_of_person(&user)
                .filter(|k| !k.revoked)
                .map(|k| {
                    (
                        k.key.id.clone(),
                        k.key.name.clone(),
                        k.project.clone(),
                        k.key.environment.clone(),
                    )
                })
                .collect();
            for rec in doc.keys.iter_mut() {
                if keys.iter().any(|(id, ..)| *id == rec.key.id) {
                    rec.revoked = true;
                }
            }
            for (id, name, project, environment) in &keys {
                doc.record(event(
                    &actor,
                    "api_key.revoke",
                    ("api_key", id, name),
                    project,
                    environment,
                ));
            }
            for team in &mut doc.teams {
                team.members.retain(|m| *m != user);
            }
            for project in &mut doc.projects {
                project.grants.retain(|g| {
                    g.subject.as_option().is_none_or(|s| {
                        !(s.id == user
                            && s.kind.as_known() == Some(pb::SubjectKind::SUBJECT_KIND_USER))
                    })
                });
            }
            doc.record(event(
                &actor,
                "member.remove",
                ("user", &member.user.id, &member.user.name),
                "",
                "",
            ));
            Ok(())
        })
        .await?;
        Response::ok(RemoveMemberResponse::default())
    }

    async fn list_invitations(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListInvitationsRequest>,
    ) -> ServiceResult<ListInvitationsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let (invitations, next_page_token) = page(
            doc.invitations.clone(),
            request.page_size,
            request.page_token,
        )?;
        Response::ok(ListInvitationsResponse {
            invitations,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_invitation(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateInvitationRequest>,
    ) -> ServiceResult<CreateInvitationResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let email = request.email.trim().to_owned();
        if email.is_empty() {
            return Err(invalid("email", "required"));
        }
        if !email.contains('@') {
            return Err(invalid("email", "not an email address"));
        }
        let role = request
            .role
            .as_known()
            .and_then(Role::from_proto)
            .unwrap_or(Role::Member);
        if role == Role::Owner && actor.role < Role::Owner {
            return Err(denied(Denied::RoleRequired {
                need: Role::Owner,
                have: Some(actor.role),
            }));
        }
        let invitation = self
            .update(|doc| {
                if doc.member_by_email(&email).is_some()
                    || doc
                        .invitations
                        .iter()
                        .any(|i| i.email.eq_ignore_ascii_case(&email))
                {
                    return Err(already_exists("invitation", &email));
                }
                let mut teams = Vec::new();
                for team in &request.teams {
                    let rec = doc.team(team).ok_or_else(|| not_found("team", team))?;
                    teams.push(rec.team.slug.clone());
                }
                let now = now_ms();
                let invitation = pb::Invitation {
                    id: new_id("inv"),
                    email: email.clone(),
                    role: role.to_proto().into(),
                    teams,
                    invited_by: actor.subject().into(),
                    created_at: timestamp(now).into(),
                    expires_at: timestamp(now + INVITATION_TTL_MS).into(),
                    ..Default::default()
                };
                doc.invitations.push(invitation.clone());
                doc.record(event(
                    &actor,
                    "invitation.create",
                    ("invitation", &invitation.id, &invitation.email),
                    "",
                    "",
                ));
                Ok(invitation)
            })
            .await?;
        Response::ok(CreateInvitationResponse {
            invitation: invitation.into(),
            ..Default::default()
        })
    }

    async fn list_teams(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListTeamsRequest>,
    ) -> ServiceResult<ListTeamsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let teams = doc.teams.iter().map(team_view).collect();
        let (teams, next_page_token) = page(teams, request.page_size, request.page_token)?;
        Response::ok(ListTeamsResponse {
            teams,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_team(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateTeamRequest>,
    ) -> ServiceResult<CreateTeamResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        if request.name.trim().is_empty() {
            return Err(invalid("name", "required"));
        }
        let slug = slug_for("", &request.name)?;
        let team = self
            .update(|doc| {
                if doc.teams.iter().any(|t| t.team.slug == slug) {
                    return Err(already_exists("team", &slug));
                }
                let team = pb::Team {
                    id: new_id("team"),
                    name: request.name.trim().to_owned(),
                    slug: slug.clone(),
                    description: request.description.clone(),
                    idp_group: request.idp_group.clone(),
                    created_at: timestamp(now_ms()).into(),
                    ..Default::default()
                };
                doc.teams.push(crate::auth::directory::TeamRec {
                    team: team.clone(),
                    members: Vec::new(),
                });
                doc.record(event(
                    &actor,
                    "team.create",
                    ("team", &team.id, &team.name),
                    "",
                    "",
                ));
                Ok(team)
            })
            .await?;
        Response::ok(CreateTeamResponse {
            team: team.into(),
            ..Default::default()
        })
    }

    async fn get_team(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetTeamRequest>,
    ) -> ServiceResult<GetTeamResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let team = request.team.to_owned();
        let rec = doc.team(&team).ok_or_else(|| not_found("team", &team))?;
        let projects = doc
            .projects
            .iter()
            .filter_map(|p| {
                p.grants
                    .iter()
                    .find(|g| {
                        g.subject.as_option().is_some_and(|s| {
                            s.kind.as_known() == Some(pb::SubjectKind::SUBJECT_KIND_TEAM)
                                && s.id == rec.team.id
                        })
                    })
                    .map(|g| pb::ProjectGrantRef {
                        project: p.project.slug.clone(),
                        role: g.role,
                        ..Default::default()
                    })
            })
            .collect();
        Response::ok(GetTeamResponse {
            team: team_view(rec).into(),
            members: rec
                .members
                .iter()
                .filter_map(|id| doc.member(id).map(|m| m.user.clone()))
                .collect(),
            projects,
            ..Default::default()
        })
    }

    async fn add_team_member(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, AddTeamMemberRequest>,
    ) -> ServiceResult<AddTeamMemberResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let (team, user) = (request.team.to_owned(), request.user.to_owned());
        self.update(|doc| {
            let Some(at) = doc
                .teams
                .iter()
                .position(|t| t.team.id == team || t.team.slug == team)
            else {
                return Err(not_found("team", &team));
            };
            if user.is_empty() {
                return Err(invalid("user", "required"));
            }
            let member = doc
                .member(&user)
                .ok_or_else(|| not_found("user", &user))?
                .clone();
            let rec = &mut doc.teams[at];
            if !rec.members.contains(&user) {
                rec.members.push(user.clone());
            }
            let (id, name) = (rec.team.id.clone(), rec.team.name.clone());
            doc.record(event(
                &actor,
                "team.member.add",
                ("team", &id, &name),
                "",
                &member.user.id,
            ));
            Ok(())
        })
        .await?;
        Response::ok(AddTeamMemberResponse::default())
    }
}

// ----- ProjectService -----

impl ProjectService for Admin {
    async fn list_projects(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListProjectsRequest>,
    ) -> ServiceResult<ListProjectsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let projects = doc.projects.iter().map(|p| project_view(&doc, p)).collect();
        let (projects, next_page_token) = page(projects, request.page_size, request.page_token)?;
        Response::ok(ListProjectsResponse {
            projects,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateProjectRequest>,
    ) -> ServiceResult<CreateProjectResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        if request.name.trim().is_empty() {
            return Err(invalid("name", "required"));
        }
        let slug = slug_for(&request.slug, &request.name)?;
        let project = self
            .update(|doc| {
                if doc.project(&slug).is_some() {
                    return Err(already_exists("project", &slug));
                }
                let project = pb::Project {
                    id: new_id("prj"),
                    slug: slug.clone(),
                    name: request.name.trim().to_owned(),
                    description: request.description.clone(),
                    created_at: timestamp(now_ms()).into(),
                    ..Default::default()
                };
                doc.projects.push(crate::auth::directory::ProjectRec {
                    project: project.clone(),
                    grants: Vec::new(),
                    environments: Vec::new(),
                });
                doc.record(event(
                    &actor,
                    "project.create",
                    ("project", &project.id, &project.name),
                    &project.slug,
                    "",
                ));
                Ok(project)
            })
            .await?;
        Response::ok(CreateProjectResponse {
            project: project.into(),
            ..Default::default()
        })
    }

    async fn get_project(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetProjectRequest>,
    ) -> ServiceResult<GetProjectResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let slug = request.project.to_owned();
        let rec = doc
            .project(&slug)
            .ok_or_else(|| not_found("project", &slug))?;
        Response::ok(GetProjectResponse {
            project: project_view(&doc, rec).into(),
            ..Default::default()
        })
    }

    async fn list_access(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListAccessRequest>,
    ) -> ServiceResult<ListAccessResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let slug = request.project.to_owned();
        let rec = doc
            .project(&slug)
            .ok_or_else(|| not_found("project", &slug))?;
        let (grants, next_page_token) =
            page(rec.grants.clone(), request.page_size, request.page_token)?;
        Response::ok(ListAccessResponse {
            grants,
            next_page_token,
            ..Default::default()
        })
    }

    async fn grant_access(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GrantAccessRequest>,
    ) -> ServiceResult<GrantAccessResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let slug = request.project.clone();
        let subject = request.subject.as_option().cloned().unwrap_or_default();
        let kind = subject.kind.as_known();
        let role = request.role;
        let grant = self
            .update(|doc| {
                if doc.project(&slug).is_none() {
                    return Err(not_found("project", &slug));
                }
                let Some(
                    kind
                    @ (pb::SubjectKind::SUBJECT_KIND_USER | pb::SubjectKind::SUBJECT_KIND_TEAM),
                ) = kind
                else {
                    return Err(invalid("subject", "a user or a team, by id"));
                };
                if subject.id.is_empty() {
                    return Err(invalid("subject", "a user or a team, by id"));
                }
                if role
                    .as_known()
                    .is_none_or(|r| r == pb::ProjectRole::PROJECT_ROLE_UNSPECIFIED)
                {
                    return Err(invalid("role", "required"));
                }
                let named = subject_named(doc, kind, &subject.id).ok_or_else(|| {
                    not_found(
                        if kind == pb::SubjectKind::SUBJECT_KIND_USER {
                            "user"
                        } else {
                            "team"
                        },
                        &subject.id,
                    )
                })?;
                let rec = doc
                    .project_mut(&slug)
                    .ok_or_else(|| not_found("project", &slug))?;
                let existing = rec.grants.iter().position(|g| {
                    g.subject
                        .as_option()
                        .is_some_and(|s| s.id == named.id && s.kind == named.kind)
                });
                let grant = match existing {
                    Some(at) => {
                        rec.grants[at].role = role;
                        rec.grants[at].clone()
                    }
                    None => {
                        let grant = pb::Grant {
                            id: new_id("grt"),
                            subject: named.clone().into(),
                            role,
                            created_at: timestamp(now_ms()).into(),
                            ..Default::default()
                        };
                        rec.grants.push(grant.clone());
                        grant
                    }
                };
                let project_id = rec.project.id.clone();
                doc.record(event(
                    &actor,
                    "project.access.grant",
                    ("project", &project_id, &named.name),
                    &slug,
                    "",
                ));
                Ok(grant)
            })
            .await?;
        Response::ok(GrantAccessResponse {
            grant: grant.into(),
            ..Default::default()
        })
    }

    async fn list_environments(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListEnvironmentsRequest>,
    ) -> ServiceResult<ListEnvironmentsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let slug = request.project.to_owned();
        let rec = doc
            .project(&slug)
            .ok_or_else(|| not_found("project", &slug))?;
        let (environments, next_page_token) = page(
            rec.environments.clone(),
            request.page_size,
            request.page_token,
        )?;
        Response::ok(ListEnvironmentsResponse {
            environments,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_environment(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateEnvironmentRequest>,
    ) -> ServiceResult<CreateEnvironmentResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let project = request.project.clone();
        if request.name.trim().is_empty() {
            if self.doc().await?.project(&project).is_none() {
                return Err(not_found("project", &project));
            }
            return Err(invalid("name", "required"));
        }
        let slug = slug_for(&request.slug, &request.name)?;
        let environment = self
            .update(|doc| {
                let rec = doc
                    .project_mut(&project)
                    .ok_or_else(|| not_found("project", &project))?;
                if rec.environments.iter().any(|e| e.slug == slug) {
                    return Err(already_exists("environment", &format!("{project}/{slug}")));
                }
                let environment = pb::Environment {
                    id: new_id("env"),
                    slug: slug.clone(),
                    name: request.name.trim().to_owned(),
                    project: project.clone(),
                    namespace: format!("{project}-{slug}"),
                    protected: request.protected,
                    created_at: timestamp(now_ms()).into(),
                    stats: pb::EnvironmentStats::default().into(),
                    ..Default::default()
                };
                rec.environments.push(environment.clone());
                doc.record(event(
                    &actor,
                    "environment.create",
                    ("environment", &environment.id, &environment.name),
                    &project,
                    &slug,
                ));
                Ok(environment)
            })
            .await?;
        Response::ok(CreateEnvironmentResponse {
            environment: environment.into(),
            ..Default::default()
        })
    }

    async fn get_environment(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetEnvironmentRequest>,
    ) -> ServiceResult<GetEnvironmentResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let found = environment(&doc, request.project, request.environment)?;
        Response::ok(GetEnvironmentResponse {
            environment: found.clone().into(),
            ..Default::default()
        })
    }

    async fn get_environment_activity(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetEnvironmentActivityRequest>,
    ) -> ServiceResult<GetEnvironmentActivityResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let found = environment(&doc, request.project, request.environment)?;
        let now = now_ms();
        Response::ok(GetEnvironmentActivityResponse {
            environment: found.slug.clone(),
            period_start: timestamp(now - 30 * 24 * 3600 * 1000).into(),
            period_end: timestamp(now).into(),
            series: Vec::new(),
            ..Default::default()
        })
    }
}

// ----- AgentService -----

impl AgentService for Admin {
    async fn list_agents(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListAgentsRequest>,
    ) -> ServiceResult<ListAgentsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let project = request.project.to_owned();
        if doc.project(&project).is_none() {
            return Err(not_found("project", &project));
        }
        let agents = doc
            .agents
            .iter()
            .filter(|a| a.agent.project == project)
            .map(|a| a.agent.clone())
            .collect();
        let (agents, next_page_token) = page(agents, request.page_size, request.page_token)?;
        Response::ok(ListAgentsResponse {
            agents,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_agent(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateAgentRequest>,
    ) -> ServiceResult<CreateAgentResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let project = request.project.clone();
        let agent = self
            .update(|doc| {
                if doc.project(&project).is_none() {
                    return Err(not_found("project", &project));
                }
                if request.name.trim().is_empty() {
                    return Err(invalid("name", "required"));
                }
                if doc
                    .agents
                    .iter()
                    .any(|a| a.agent.project == project && a.agent.name == request.name.trim())
                {
                    return Err(already_exists("agent", request.name.trim()));
                }
                if request.max_token_ttl_s < 0 {
                    return Err(invalid("max_token_ttl_s", "must not be negative"));
                }
                let owner = match request.owner.as_option() {
                    Some(owner) if !owner.id.is_empty() => {
                        let kind = owner
                            .kind
                            .as_known()
                            .filter(|k| {
                                matches!(
                                    k,
                                    pb::SubjectKind::SUBJECT_KIND_USER
                                        | pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT
                                )
                            })
                            .ok_or_else(|| invalid("owner", "a user or a service account"))?;
                        subject_named(doc, kind, &owner.id)
                            .ok_or_else(|| not_found("owner", &owner.id))?
                    }
                    _ => actor.subject(),
                };
                let agent = pb::Agent {
                    id: new_id("agt"),
                    name: request.name.trim().to_owned(),
                    description: request.description.clone(),
                    project: project.clone(),
                    owner: owner.into(),
                    status: pb::AgentStatus::AGENT_STATUS_ACTIVE.into(),
                    policy: request.policy.clone(),
                    max_token_ttl_s: match request.max_token_ttl_s {
                        0 => DEFAULT_MAX_TOKEN_TTL_S,
                        n => n,
                    },
                    limits: request.limits.clone(),
                    created_at: timestamp(now_ms()).into(),
                    ..Default::default()
                };
                doc.agents.push(crate::auth::directory::AgentRec {
                    agent: agent.clone(),
                    trust_policies: Vec::new(),
                });
                doc.record(event(
                    &actor,
                    "agent.create",
                    ("agent", &agent.id, &agent.name),
                    &project,
                    "",
                ));
                Ok(agent)
            })
            .await?;
        Response::ok(CreateAgentResponse {
            agent: agent.into(),
            ..Default::default()
        })
    }

    async fn get_agent(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, GetAgentRequest>,
    ) -> ServiceResult<GetAgentResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let id = request.agent.to_owned();
        let rec = doc.agent(&id).ok_or_else(|| not_found("agent", &id))?;
        Response::ok(GetAgentResponse {
            agent: rec.agent.clone().into(),
            ..Default::default()
        })
    }

    async fn update_agent(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, UpdateAgentRequest>,
    ) -> ServiceResult<UpdateAgentResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let changes = request.agent.as_option().cloned().unwrap_or_default();
        let paths = mask_paths(&request.update_mask);
        let agent = self
            .update(|doc| {
                let rec = doc
                    .agent_mut(&changes.id)
                    .ok_or_else(|| not_found("agent", &changes.id))?;
                let mut agent = rec.agent.clone();
                for path in &paths {
                    match path.as_str() {
                        "description" => agent.description.clone_from(&changes.description),
                        "policy" => agent.policy.clone_from(&changes.policy),
                        "max_token_ttl_s" | "maxTokenTtlS" => {
                            if changes.max_token_ttl_s < 0 {
                                return Err(invalid("max_token_ttl_s", "must not be negative"));
                            }
                            agent.max_token_ttl_s = changes.max_token_ttl_s;
                        }
                        "limits" => agent.limits = changes.limits.clone(),
                        other => {
                            return Err(invalid(
                                "update_mask",
                                &format!("`{other}` cannot be changed"),
                            ));
                        }
                    }
                }
                if !paths.is_empty() {
                    rec.agent = agent.clone();
                    doc.record(event(
                        &actor,
                        "agent.update",
                        ("agent", &agent.id, &agent.name),
                        &agent.project,
                        "",
                    ));
                }
                Ok(agent)
            })
            .await?;
        Response::ok(UpdateAgentResponse {
            agent: agent.into(),
            ..Default::default()
        })
    }

    async fn suspend_agent(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, SuspendAgentRequest>,
    ) -> ServiceResult<SuspendAgentResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let agent = self
            .set_status(
                &actor,
                request.agent,
                pb::AgentStatus::AGENT_STATUS_SUSPENDED,
            )
            .await?;
        Response::ok(SuspendAgentResponse {
            agent: agent.into(),
            ..Default::default()
        })
    }

    async fn resume_agent(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ResumeAgentRequest>,
    ) -> ServiceResult<ResumeAgentResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let agent = self
            .set_status(&actor, request.agent, pb::AgentStatus::AGENT_STATUS_ACTIVE)
            .await?;
        Response::ok(ResumeAgentResponse {
            agent: agent.into(),
            ..Default::default()
        })
    }

    async fn list_agent_tokens(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListAgentTokensRequest>,
    ) -> ServiceResult<ListAgentTokensResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let id = request.agent.to_owned();
        doc.agent(&id).ok_or_else(|| not_found("agent", &id))?;
        // Agent tokens are minted by MT1's issuer; none exist before it.
        let (tokens, next_page_token) = page(
            Vec::<pb::AgentToken>::new(),
            request.page_size,
            request.page_token,
        )?;
        Response::ok(ListAgentTokensResponse {
            tokens,
            next_page_token,
            ..Default::default()
        })
    }

    async fn list_trust_policies(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListTrustPoliciesRequest>,
    ) -> ServiceResult<ListTrustPoliciesResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let id = request.agent.to_owned();
        let rec = doc.agent(&id).ok_or_else(|| not_found("agent", &id))?;
        let (trust_policies, next_page_token) = page(
            rec.trust_policies.clone(),
            request.page_size,
            request.page_token,
        )?;
        Response::ok(ListTrustPoliciesResponse {
            trust_policies,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_trust_policy(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateTrustPolicyRequest>,
    ) -> ServiceResult<CreateTrustPolicyResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let policy = self
            .update(|doc| {
                let project = doc
                    .agent(&request.agent)
                    .ok_or_else(|| not_found("agent", &request.agent))?
                    .agent
                    .project
                    .clone();
                for (field, value) in [
                    ("name", &request.name),
                    ("issuer", &request.issuer),
                    ("subject_pattern", &request.subject_pattern),
                ] {
                    if value.is_empty() {
                        return Err(invalid(field, "required"));
                    }
                }
                for environment in &request.environments {
                    let known = doc
                        .project(&project)
                        .is_some_and(|p| p.environments.iter().any(|e| e.slug == *environment));
                    if !known {
                        return Err(not_found(
                            "environment",
                            &format!("{project}/{environment}"),
                        ));
                    }
                }
                let policy = pb::TrustPolicy {
                    id: new_id("tp"),
                    agent: request.agent.clone(),
                    name: request.name.clone(),
                    issuer: request.issuer.clone(),
                    audience: request.audience.clone(),
                    subject_pattern: request.subject_pattern.clone(),
                    environments: request.environments.clone(),
                    created_at: timestamp(now_ms()).into(),
                    ..Default::default()
                };
                let rec = doc
                    .agent_mut(&request.agent)
                    .ok_or_else(|| not_found("agent", &request.agent))?;
                rec.trust_policies.push(policy.clone());
                doc.record(event(
                    &actor,
                    "trust_policy.create",
                    ("trust_policy", &policy.id, &policy.name),
                    &project,
                    "",
                ));
                Ok(policy)
            })
            .await?;
        Response::ok(CreateTrustPolicyResponse {
            trust_policy: policy.into(),
            ..Default::default()
        })
    }

    async fn list_service_accounts(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListServiceAccountsRequest>,
    ) -> ServiceResult<ListServiceAccountsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let project = request.project.to_owned();
        if doc.project(&project).is_none() {
            return Err(not_found("project", &project));
        }
        let accounts = doc
            .service_accounts
            .iter()
            .filter(|s| s.project == project)
            .map(|s| service_account_view(&doc, s))
            .collect();
        let (service_accounts, next_page_token) =
            page(accounts, request.page_size, request.page_token)?;
        Response::ok(ListServiceAccountsResponse {
            service_accounts,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_service_account(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateServiceAccountRequest>,
    ) -> ServiceResult<CreateServiceAccountResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        let project = request.project.clone();
        let account = self
            .update(|doc| {
                if doc.project(&project).is_none() {
                    return Err(not_found("project", &project));
                }
                let name = request.name.trim();
                if name.is_empty() {
                    return Err(invalid("name", "required"));
                }
                if doc
                    .service_accounts
                    .iter()
                    .any(|s| s.project == project && s.name == name)
                {
                    return Err(already_exists("service account", name));
                }
                let account = pb::ServiceAccount {
                    id: new_id("sa"),
                    name: name.to_owned(),
                    description: request.description.clone(),
                    project: project.clone(),
                    created_at: timestamp(now_ms()).into(),
                    ..Default::default()
                };
                doc.service_accounts.push(account.clone());
                doc.record(event(
                    &actor,
                    "service_account.create",
                    ("service_account", &account.id, &account.name),
                    &project,
                    "",
                ));
                Ok(account)
            })
            .await?;
        Response::ok(CreateServiceAccountResponse {
            service_account: account.into(),
            ..Default::default()
        })
    }
}

impl Admin {
    /// Suspends or resumes an agent (the same status twice is no change).
    async fn set_status(
        &self,
        actor: &Actor,
        id: &str,
        status: pb::AgentStatus,
    ) -> Result<pb::Agent, ConnectError> {
        let action = match status {
            pb::AgentStatus::AGENT_STATUS_SUSPENDED => "agent.suspend",
            _ => "agent.resume",
        };
        self.update(|doc| {
            let rec = doc.agent_mut(id).ok_or_else(|| not_found("agent", id))?;
            rec.agent.status = status.into();
            let agent = rec.agent.clone();
            doc.record(event(
                actor,
                action,
                ("agent", &agent.id, &agent.name),
                &agent.project,
                "",
            ));
            Ok(agent)
        })
        .await
    }
}

// ----- KeyService -----

impl KeyService for Admin {
    async fn list_keys(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListKeysRequest>,
    ) -> ServiceResult<ListKeysResponse> {
        self.actor(&ctx, Role::Member).await?;
        let doc = self.doc().await?;
        let (project, env) = (request.project.to_owned(), request.environment.to_owned());
        environment(&doc, &project, &env)?;
        let keys = doc
            .keys
            .iter()
            .filter(|k| !k.revoked && k.project == project && k.key.environment == env)
            .map(|k| k.key.clone())
            .collect();
        let (keys, next_page_token) = page(keys, request.page_size, request.page_token)?;
        Response::ok(ListKeysResponse {
            keys,
            next_page_token,
            ..Default::default()
        })
    }

    async fn create_key(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, CreateKeyRequest>,
    ) -> ServiceResult<CreateKeyResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let request = request.to_owned_message();
        environment(&*self.doc().await?, &request.project, &request.environment)?;
        if request.idempotency_key.is_empty() {
            return Err(invalid(
                "idempotency_key",
                "required: a key's secret is answered once, so a retry must be recognisable",
            ));
        }
        if request.name.trim().is_empty() {
            return Err(invalid("name", "required"));
        }
        if request.expires_in_days < 0 {
            return Err(invalid("expires_in_days", "must not be negative"));
        }
        // argon2id is slow: hash once, outside the store's write.
        let (secret, id) = api_key::generate();
        let hash = tokio::task::spawn_blocking({
            let secret = secret.clone();
            move || api_key::hash_key(&secret)
        })
        .await
        .map_err(|err| unavailable(&format!("hashing the key: {err}")))?;
        let key = self
            .update(|doc| {
                if doc.keys.iter().any(|k| {
                    k.idempotency_key == request.idempotency_key && k.created_by == actor.id
                }) {
                    return Err(refuse(
                        ErrorCode::FailedPrecondition,
                        "secret_already_issued",
                        "this idempotency_key already created a key, and its secret is answered \
                         once; create a new key with a new idempotency_key",
                        &[],
                    ));
                }
                let (principal, role) = if request.service_account.is_empty() {
                    (actor.subject(), actor.role)
                } else {
                    let account = doc
                        .service_accounts
                        .iter()
                        .find(|s| s.id == request.service_account && s.project == request.project)
                        .ok_or_else(|| not_found("service account", &request.service_account))?;
                    (
                        pb::SubjectRef {
                            kind: pb::SubjectKind::SUBJECT_KIND_SERVICE_ACCOUNT.into(),
                            id: account.id.clone(),
                            name: account.name.clone(),
                            ..Default::default()
                        },
                        Role::Member,
                    )
                };
                let now = now_ms();
                let key = pb::ApiKey {
                    id: id.clone(),
                    name: request.name.trim().to_owned(),
                    prefix: secret.chars().take(11).collect(),
                    environment: request.environment.clone(),
                    principal: principal.into(),
                    scopes: request.scopes.clone(),
                    created_at: timestamp(now).into(),
                    expires_at: match request.expires_in_days {
                        0 => Default::default(),
                        days => timestamp(now + i64::from(days) * 24 * 3600 * 1000).into(),
                    },
                    ..Default::default()
                };
                doc.keys.push(KeyRec {
                    key: key.clone(),
                    project: request.project.clone(),
                    hash: hash.clone(),
                    role,
                    idempotency_key: request.idempotency_key.clone(),
                    created_by: actor.id.clone(),
                    revoked: false,
                });
                doc.record(event(
                    &actor,
                    "api_key.create",
                    ("api_key", &key.id, &key.name),
                    &request.project,
                    &request.environment,
                ));
                Ok(key)
            })
            .await?;
        Response::ok(CreateKeyResponse {
            key: key.into(),
            secret,
            ..Default::default()
        })
    }

    async fn revoke_key(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RevokeKeyRequest>,
    ) -> ServiceResult<RevokeKeyResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let id = request.key.to_owned();
        self.update(|doc| {
            let rec = doc
                .keys
                .iter_mut()
                .find(|k| k.key.id == id)
                .ok_or_else(|| not_found("key", &id))?;
            rec.revoked = true;
            let (name, project, environment) = (
                rec.key.name.clone(),
                rec.project.clone(),
                rec.key.environment.clone(),
            );
            doc.record(event(
                &actor,
                "api_key.revoke",
                ("api_key", &id, &name),
                &project,
                &environment,
            ));
            Ok(())
        })
        .await?;
        Response::ok(RevokeKeyResponse::default())
    }

    async fn revoke_token(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, RevokeTokenRequest>,
    ) -> ServiceResult<RevokeTokenResponse> {
        let actor = self.actor(&ctx, Role::Admin).await?;
        let jti = request.jti.to_owned();
        if jti.is_empty() {
            return Err(invalid("jti", "required"));
        }
        self.update(|doc| {
            doc.revoke_token(&jti);
            doc.record(event(&actor, "token.revoke", ("token", &jti, &jti), "", ""));
            Ok(())
        })
        .await?;
        Response::ok(RevokeTokenResponse::default())
    }
}

// ----- AuditService -----

/// Which events a list or watch selects.
#[derive(Debug, Clone)]
struct Filter {
    project: String,
    actor: String,
}

impl Filter {
    fn matches(&self, event: &pb::AuditEvent) -> bool {
        (self.project.is_empty() || event.project == self.project)
            && (self.actor.is_empty()
                || event
                    .actor
                    .as_option()
                    .is_some_and(|actor| actor.id == self.actor))
    }
}

fn cursor(seq: u64) -> String {
    format!("a{seq}")
}

fn parse_cursor(cursor: &str) -> Option<u64> {
    cursor.strip_prefix('a')?.parse().ok()
}

fn snapshot(doc: &Doc, filter: &Filter, size: usize, reset: bool) -> WatchAuditEventsResponse {
    WatchAuditEventsResponse {
        event: Some(WatchEvent::Snapshot(Box::new(pb::AuditSnapshot {
            events: doc
                .audit
                .iter()
                .rev()
                .map(|r| &r.event)
                .filter(|e| filter.matches(e))
                .take(size)
                .cloned()
                .collect(),
            ..Default::default()
        }))),
        cursor: cursor(doc.audit_seq),
        snapshot_reset: reset,
        ..Default::default()
    }
}

fn append(record: AuditRec) -> WatchAuditEventsResponse {
    WatchAuditEventsResponse {
        event: Some(WatchEvent::Append(Box::new(record.event))),
        cursor: cursor(record.seq),
        ..Default::default()
    }
}

/// The events after `cursor`, oldest first, if the log still holds them.
fn since(doc: &Doc, cursor: u64) -> Option<Vec<AuditRec>> {
    if cursor > doc.audit_seq {
        return None;
    }
    let oldest_held = doc.audit.first().map_or(doc.audit_seq, |r| r.seq - 1);
    if cursor < oldest_held {
        return None;
    }
    Some(
        doc.audit
            .iter()
            .filter(|r| r.seq > cursor)
            .cloned()
            .collect(),
    )
}

impl AuditService for Admin {
    async fn list_audit_events(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, ListAuditEventsRequest>,
    ) -> ServiceResult<ListAuditEventsResponse> {
        self.actor(&ctx, Role::Member).await?;
        let request = request.to_owned_message();
        let filter = Filter {
            project: request.project,
            actor: request.actor,
        };
        let doc = self.doc().await?;
        let events = doc
            .audit
            .iter()
            .rev()
            .map(|r| &r.event)
            .filter(|e| filter.matches(e))
            .cloned()
            .collect();
        let (events, next_page_token) = page(events, request.page_size, &request.page_token)?;
        Response::ok(ListAuditEventsResponse {
            events,
            next_page_token,
            ..Default::default()
        })
    }

    async fn watch_audit_events(
        &self,
        ctx: RequestContext,
        request: ServiceRequest<'_, WatchAuditEventsRequest>,
    ) -> ServiceResult<ServiceStream<WatchAuditEventsResponse>> {
        self.actor(&ctx, Role::Member).await?;
        let request = request.to_owned_message();
        let size = match usize::try_from(request.snapshot_size) {
            Ok(0) => DEFAULT_SNAPSHOT,
            Ok(n) => n.min(MAX_SNAPSHOT),
            Err(_) => return Err(invalid("snapshot_size", "must not be negative")),
        };
        let filter = Filter {
            project: request.project,
            actor: request.actor,
        };
        // Subscribe before reading, so no event falls between the snapshot
        // (or the replay) and the live stream; `seq` drops the overlap.
        let rx = self.principals.directory.subscribe();
        let doc = self.doc().await?;
        let asked = !request.resume_cursor.is_empty();
        let mut queue = VecDeque::new();
        match parse_cursor(&request.resume_cursor).and_then(|c| since(&doc, c)) {
            Some(missed) if asked => queue.extend(
                missed
                    .into_iter()
                    .filter(|r| filter.matches(&r.event))
                    .map(append),
            ),
            _ => queue.push_back(snapshot(&doc, &filter, size, asked)),
        }
        let last_seq = doc.audit_seq;
        let period = self.heartbeat;
        let timer = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        let directory = self.principals.directory.clone();
        let stream = futures::stream::unfold(
            (queue, rx, timer, last_seq, filter, directory),
            move |(mut queue, mut rx, mut timer, mut last_seq, filter, directory)| async move {
                loop {
                    if let Some(item) = queue.pop_front() {
                        return Some((Ok(item), (queue, rx, timer, last_seq, filter, directory)));
                    }
                    tokio::select! {
                        change = rx.recv() => match change {
                            Ok(record) if record.seq > last_seq => {
                                last_seq = record.seq;
                                if filter.matches(&record.event) {
                                    queue.push_back(append(record));
                                }
                            }
                            Ok(_) => {}
                            Err(RecvError::Lagged(_)) => match directory.read().await {
                                // Too far behind: a fresh snapshot.
                                Ok(doc) => {
                                    last_seq = doc.audit_seq;
                                    queue.push_back(snapshot(&doc, &filter, size, true));
                                }
                                Err(err) => {
                                    let err: ConnectError = err.into();
                                    return Some((Err(err), (queue, rx, timer, last_seq, filter, directory)));
                                }
                            },
                            Err(RecvError::Closed) => return None,
                        },
                        _ = timer.tick() => {
                            queue.push_back(WatchAuditEventsResponse {
                                event: Some(WatchEvent::Heartbeat(Box::default())),
                                cursor: cursor(last_seq),
                                ..Default::default()
                            });
                        }
                    }
                }
            },
        );
        Response::stream_ok(stream)
    }
}

/// Registers the five `loams.admin.v1` services on `router`.
pub(super) fn register(router: connectrpc::Router, principals: &Principals) -> connectrpc::Router {
    let admin = Arc::new(Admin {
        principals: principals.clone(),
        heartbeat: HEARTBEAT,
    });
    let router = OrgServiceExt::register(admin.clone(), router);
    let router = ProjectServiceExt::register(admin.clone(), router);
    let router = AgentServiceExt::register(admin.clone(), router);
    let router = KeyServiceExt::register(admin.clone(), router);
    AuditServiceExt::register(admin, router)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_lower_case_with_inner_hyphens() {
        assert_eq!(slugify("  Docs Search!! v2 "), "docs-search-v2");
        assert_eq!(slugify("***"), "");
        assert!(slug_for("", "***").is_err());
        assert_eq!(slug_for("prod-eu", "x").unwrap(), "prod-eu");
        assert!(slug_for("Prod", "x").is_err());
        assert!(slug_for("-prod", "x").is_err());
    }

    #[test]
    fn pages_walk_the_whole_list_and_refuse_a_foreign_token() {
        let items: Vec<u32> = (0..5).collect();
        let (first, next) = page(items.clone(), 2, "").unwrap();
        assert_eq!((first, next.as_str()), (vec![0, 1], "o2"));
        let (last, next) = page(items.clone(), 4, &next).unwrap();
        assert_eq!((last, next.as_str()), (vec![2, 3, 4], ""));
        assert!(page(items.clone(), 2, "o9").is_err());
        assert!(page(items, -1, "").is_err());
    }

    #[test]
    fn a_cursor_the_log_no_longer_holds_resets() {
        let mut doc = Doc::default();
        for _ in 0..3 {
            doc.record(pb::AuditEvent::default());
        }
        assert_eq!(since(&doc, 1).unwrap().len(), 2);
        assert_eq!(since(&doc, 3).unwrap().len(), 0);
        assert!(since(&doc, 4).is_none());
        doc.audit.remove(0);
        assert!(since(&doc, 0).is_none());
        assert_eq!(since(&doc, 1).unwrap().len(), 2);
    }
}
