//! marketplace.proto (MK1 Task 4b): `MarketplaceService`.
//!
//! The ndjson wire keeps the listing schema's strings (`mcp-server`,
//! `mastra`, `core`); the proto has `loams.marketplace.v1` enums, named as
//! listing.proto says: the string upper-cased, `-` as `_`, after the enum's
//! prefix (`"mcp-server"` is `KIND_MCP_SERVER`). Listing parts (the listing,
//! its Place, models, cost and provenance) are JSON on the ndjson wire and
//! messages here, converted through proto3 JSON with the enum strings
//! renamed by the schema's `x-proto-enum` annotations.

use buffa::{EnumValue, Enumeration};
use loams_agentd_proto as rs;
use loams_agentd_proto::marketplace as rs_market;
use serde_json::Value;

use crate::map::{MapError, Wire};
use crate::proto::loams::agentd::v1 as pb;
use crate::proto::loams::marketplace::v1 as listing_pb;

/// The listing schema, for its `x-proto-enum` annotations.
const SCHEMA: &str = include_str!("../../../../schemas/marketplace/listing.v1.json");

fn schema() -> &'static Value {
    static SCHEMA_VALUE: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
    SCHEMA_VALUE.get_or_init(|| serde_json::from_str(SCHEMA).unwrap_or(Value::Null))
}

/// `FrameworkName` → `FRAMEWORK_NAME_`.
fn prefix_of(enum_name: &str) -> String {
    let mut out = String::new();
    for (i, ch) in enum_name.chars().enumerate() {
        if ch.is_ascii_uppercase() && i > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_uppercase());
    }
    out.push('_');
    out
}

/// `mcp-server` → `MCP_SERVER`.
fn upper(value: &str) -> String {
    value.to_ascii_uppercase().replace('-', "_")
}

/// The unspecified value (0), as decoding would yield it.
fn unspecified<E: Enumeration>() -> EnumValue<E> {
    E::from_i32(0).map_or(EnumValue::Unknown(0), EnumValue::Known)
}

/// A schema string as a proto enum value; an unknown one is unspecified (0).
pub fn enum_from_str<E: Enumeration>(prefix: &str, value: &str) -> EnumValue<E> {
    E::from_proto_name(&format!("{prefix}{}", upper(value)))
        .map_or_else(unspecified, EnumValue::Known)
}

/// A proto enum value as its schema string; unspecified is `None`.
pub fn str_from_enum<E: Enumeration>(prefix: &str, value: EnumValue<E>) -> Option<String> {
    match value {
        EnumValue::Known(known) => {
            let rest = known.proto_name().strip_prefix(prefix)?;
            (rest != "UNSPECIFIED").then(|| rest.to_ascii_lowercase().replace('_', "-"))
        }
        EnumValue::Unknown(_) => None,
    }
}

fn opt_enum<E: Enumeration>(prefix: &str, value: Option<&String>) -> EnumValue<E> {
    value.map_or_else(unspecified, |v| enum_from_str(prefix, v))
}

/// Resolves a local `$ref`.
fn resolve(node: &Value) -> &Value {
    match node.get("$ref").and_then(Value::as_str) {
        Some(reference) => reference
            .strip_prefix("#/")
            .map(|path| {
                path.split('/')
                    .fold(schema(), |node, key| node.get(key).unwrap_or(&Value::Null))
            })
            .unwrap_or(&Value::Null),
        None => node,
    }
}

/// Rewrites `value` (shaped by schema `node`) for proto3 JSON: each enum
/// string becomes its proto name, and `$`-keys (`$schema`) are dropped.
fn to_proto_json(value: &Value, node: &Value) -> Value {
    let node = resolve(node);
    if let Some(name) = node.get("x-proto-enum").and_then(Value::as_str)
        && let Some(text) = value.as_str()
    {
        return Value::String(format!("{}{}", prefix_of(name), upper(text)));
    }
    match value {
        Value::Object(map) => {
            let properties = node.get("properties");
            // A free-form object (`google.protobuf.Struct`) is kept whole.
            let keep = node.get("x-proto-message").is_none();
            let mut out = serde_json::Map::new();
            for (key, item) in map {
                // A message has no `$schema` field; a free-form object keeps it.
                if key.starts_with('$') && !keep {
                    continue;
                }
                let child = properties.and_then(|p| p.get(key)).unwrap_or(&Value::Null);
                out.insert(
                    key.clone(),
                    if keep {
                        item.clone()
                    } else {
                        to_proto_json(item, child)
                    },
                );
            }
            Value::Object(out)
        }
        Value::Array(items) => {
            let child = node.get("items").unwrap_or(&Value::Null);
            Value::Array(
                items
                    .iter()
                    .map(|item| to_proto_json(item, child))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

/// Rewrites proto3 JSON (`T`'s serde form) back to the schema's strings.
fn from_proto_json(value: &Value, node: &Value) -> Value {
    let node = resolve(node);
    if let Some(name) = node.get("x-proto-enum").and_then(Value::as_str)
        && let Some(text) = value.as_str()
    {
        let rest = text.strip_prefix(&prefix_of(name)).unwrap_or(text);
        // The schema's own spelling (`linux-x86_64` keeps its underscore).
        let known = node
            .get("enum")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .find(|value| upper(value) == rest);
        return Value::String(match known {
            Some(value) => value.to_owned(),
            None => rest.to_ascii_lowercase().replace('_', "-"),
        });
    }
    match value {
        Value::Object(map) => {
            let properties = node.get("properties");
            let keep = node.get("x-proto-message").is_none();
            Value::Object(
                map.iter()
                    .map(|(key, item)| {
                        // proto3 JSON writes lowerCamelCase; the schema is snake_case.
                        let key = if keep { key.clone() } else { snake(key) };
                        let child = properties.and_then(|p| p.get(&key)).unwrap_or(&Value::Null);
                        let item = if keep {
                            item.clone()
                        } else {
                            from_proto_json(item, child)
                        };
                        (key, item)
                    })
                    .collect(),
            )
        }
        Value::Array(items) => {
            let child = node.get("items").unwrap_or(&Value::Null);
            Value::Array(
                items
                    .iter()
                    .map(|item| from_proto_json(item, child))
                    .collect(),
            )
        }
        other => other.clone(),
    }
}

fn snake(key: &str) -> String {
    let mut out = String::new();
    for ch in key.chars() {
        if ch.is_ascii_uppercase() {
            out.push('_');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

/// The schema node of a top-level listing property (`place`, `models`, …),
/// or the whole listing for `None`.
fn node_of(property: Option<&str>) -> &'static Value {
    match property {
        None => schema(),
        Some(name) => schema()
            .get("properties")
            .and_then(|p| p.get(name))
            .unwrap_or(&Value::Null),
    }
}

/// A listing part (JSON) as its message; JSON null is the default message.
fn part_to_pb<T: serde::de::DeserializeOwned + Default>(
    value: &Value,
    property: Option<&str>,
) -> T {
    if value.is_null() {
        return T::default();
    }
    serde_json::from_value(to_proto_json(value, node_of(property))).unwrap_or_else(|error| {
        tracing::warn!(%error, ?property, "a listing part does not map to its message");
        T::default()
    })
}

/// A listing part's message as JSON; the default message is JSON null.
fn part_from_pb<T: serde::Serialize + Default + PartialEq>(
    message: &T,
    property: Option<&str>,
) -> Result<Value, MapError> {
    if *message == T::default() {
        return Ok(Value::Null);
    }
    let value = serde_json::to_value(message).map_err(|e| MapError(e.to_string()))?;
    Ok(from_proto_json(&value, node_of(property)))
}

fn none_if_empty(value: String) -> Option<String> {
    (!value.is_empty()).then_some(value)
}

const KIND: &str = "KIND_";
const FRAMEWORK: &str = "FRAMEWORK_NAME_";
const LANGUAGE: &str = "LANGUAGE_";
const LEVEL: &str = "CONFORMANCE_LEVEL_";
const RISK: &str = "RISK_";

wire_enum!(rs_market::ListingTier => pb::ListingTier {
    FirstParty => LISTING_TIER_FIRST_PARTY,
    Verified => LISTING_TIER_VERIFIED,
    Community => LISTING_TIER_COMMUNITY,
});

wire_enum!(rs_market::RevocationSeverity => pb::RevocationSeverity {
    Warn => REVOCATION_SEVERITY_WARN,
    Disable => REVOCATION_SEVERITY_DISABLE,
});

impl Wire for rs_market::MarketplaceSource {
    type Pb = pb::MarketplaceSource;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::MarketplaceSource {
            name,
            url,
            identity,
            issuer,
            official,
            refreshed_ms,
            error,
            listings,
        } = self;
        pb::MarketplaceSource {
            name: name.clone(),
            url: url.clone(),
            identity: identity.clone(),
            issuer: issuer.clone(),
            official: *official,
            refreshed_ms: *refreshed_ms,
            error: error.clone().unwrap_or_default(),
            listings: i32::try_from(*listings).unwrap_or(i32::MAX),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            name: pb.name,
            url: pb.url,
            identity: pb.identity,
            issuer: pb.issuer,
            official: pb.official,
            refreshed_ms: pb.refreshed_ms,
            error: none_if_empty(pb.error),
            listings: u32::try_from(pb.listings).map_err(|e| MapError(e.to_string()))?,
        })
    }
}

wire_struct!(rs_market::MarketplaceSourceList => pb::ListSourcesResponse {
    sources => sources,
});

impl Wire for rs_market::AddMarketplaceSourceParams {
    type Pb = pb::AddSourceRequest;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::AddMarketplaceSourceParams {
            name,
            url,
            identity,
            issuer,
        } = self;
        pb::AddSourceRequest {
            name: name.clone(),
            url: url.clone(),
            identity: identity.clone(),
            issuer: issuer.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            name: pb.name,
            url: pb.url,
            identity: pb.identity,
            issuer: none_if_empty(pb.issuer),
        })
    }
}

wire_struct!(rs_market::RemoveMarketplaceSourceParams => pb::RemoveSourceRequest {
    name => name,
});

wire_struct!(rs_market::ListingRevocation => pb::Revocation {
    severity => severity,
    reason => reason,
    versions => versions,
    advisory => advisory as opt_string,
});

impl Wire for rs_market::SearchListingsParams {
    type Pb = pb::SearchListingsRequest;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::SearchListingsParams {
            query,
            kinds,
            frameworks,
            languages,
            min_level,
            tiers,
            source,
        } = self;
        pb::SearchListingsRequest {
            query: query.clone(),
            kinds: kinds.iter().map(|k| enum_from_str(KIND, k)).collect(),
            frameworks: frameworks
                .iter()
                .map(|f| enum_from_str(FRAMEWORK, f))
                .collect(),
            languages: languages
                .iter()
                .map(|l| enum_from_str(LANGUAGE, l))
                .collect(),
            min_level: opt_enum(LEVEL, min_level.as_ref()),
            tiers: tiers.to_pb(),
            source: source.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            query: pb.query,
            kinds: pb
                .kinds
                .into_iter()
                .map(|v| {
                    let raw = v.to_i32();
                    str_from_enum(KIND, v).ok_or_else(|| MapError::enum_value("Kind", raw))
                })
                .collect::<Result<_, _>>()?,
            frameworks: pb
                .frameworks
                .into_iter()
                .map(|v| {
                    let raw = v.to_i32();
                    str_from_enum(FRAMEWORK, v)
                        .ok_or_else(|| MapError::enum_value("FrameworkName", raw))
                })
                .collect::<Result<_, _>>()?,
            languages: pb
                .languages
                .into_iter()
                .map(|v| {
                    let raw = v.to_i32();
                    str_from_enum(LANGUAGE, v).ok_or_else(|| MapError::enum_value("Language", raw))
                })
                .collect::<Result<_, _>>()?,
            min_level: str_from_enum(LEVEL, pb.min_level),
            tiers: Wire::from_pb(pb.tiers)?,
            source: none_if_empty(pb.source),
        })
    }
}

impl Wire for rs_market::ListingSummary {
    type Pb = pb::ListingSummary;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::ListingSummary {
            id,
            source,
            kind,
            title,
            summary,
            tier,
            latest,
            framework,
            language,
            level,
            revocation,
        } = self;
        pb::ListingSummary {
            id: id.clone(),
            source: source.clone(),
            kind: enum_from_str(KIND, kind),
            title: title.clone(),
            summary: summary.clone(),
            tier: tier.to_pb(),
            latest: latest.clone().unwrap_or_default(),
            framework: opt_enum(FRAMEWORK, framework.as_ref()),
            language: opt_enum(LANGUAGE, language.as_ref()),
            level: opt_enum(LEVEL, level.as_ref()),
            revocation: crate::map::opt_msg(revocation),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let kind_raw = pb.kind.to_i32();
        Ok(Self {
            id: pb.id,
            source: pb.source,
            kind: str_from_enum(KIND, pb.kind)
                .ok_or_else(|| MapError::enum_value("Kind", kind_raw))?,
            title: pb.title,
            summary: pb.summary,
            tier: Wire::from_pb(pb.tier)?,
            latest: none_if_empty(pb.latest),
            framework: str_from_enum(FRAMEWORK, pb.framework),
            language: str_from_enum(LANGUAGE, pb.language),
            level: str_from_enum(LEVEL, pb.level),
            revocation: crate::map::from_opt_msg(pb.revocation)?,
        })
    }
}

wire_struct!(rs_market::ListingSearchResults => pb::SearchListingsResponse {
    results => results,
});

impl Wire for rs_market::ListingRef {
    type Pb = pb::GetListingRequest;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::ListingRef {
            id,
            version,
            source,
        } = self;
        pb::GetListingRequest {
            id: id.clone(),
            version: version.clone().unwrap_or_default(),
            source: source.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            id: pb.id,
            version: none_if_empty(pb.version),
            source: none_if_empty(pb.source),
        })
    }
}

/// `PlanInstallRequest` carries the same three fields.
pub fn listing_ref_from_plan(pb: pb::PlanInstallRequest) -> rs_market::ListingRef {
    rs_market::ListingRef {
        id: pb.id,
        version: none_if_empty(pb.version),
        source: none_if_empty(pb.source),
    }
}

/// The proto request of a [`rs_market::ListingRef`] for `PlanInstall`.
pub fn plan_request_of(params: &rs_market::ListingRef) -> pb::PlanInstallRequest {
    pb::PlanInstallRequest {
        id: params.id.clone(),
        version: params.version.clone().unwrap_or_default(),
        source: params.source.clone().unwrap_or_default(),
        ..Default::default()
    }
}

wire_struct!(rs_market::ListingVersion => pb::ListingVersion {
    version => version,
    yanked => yanked as opt_string,
    revocation => revocation as opt_msg,
});

impl Wire for rs_market::ListingDetail {
    type Pb = pb::GetListingResponse;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::ListingDetail {
            summary,
            listing,
            versions,
        } = self;
        pb::GetListingResponse {
            summary: crate::map::msg(summary),
            listing: buffa::MessageField::some(part_to_pb::<listing_pb::Listing>(listing, None)),
            versions: versions.to_pb(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            summary: crate::map::from_msg(pb.summary, "summary")?,
            listing: part_from_pb(&pb.listing.into_option().unwrap_or_default(), None)?,
            versions: Wire::from_pb(pb.versions)?,
        })
    }
}

impl Wire for rs_market::PlanTool {
    type Pb = pb::PlanTool;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::PlanTool {
            name,
            description,
            risk,
            always_asks,
            mcp_server,
        } = self;
        pb::PlanTool {
            name: name.clone(),
            description: description.clone(),
            risk: enum_from_str(RISK, risk),
            always_asks: *always_asks,
            mcp_server: mcp_server.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let raw = pb.risk.to_i32();
        Ok(Self {
            name: pb.name,
            description: pb.description,
            risk: str_from_enum(RISK, pb.risk).ok_or_else(|| MapError::enum_value("Risk", raw))?,
            always_asks: pb.always_asks,
            mcp_server: none_if_empty(pb.mcp_server),
        })
    }
}

wire_struct!(rs_market::PlanSecret => pb::PlanSecret {
    name => name,
    description => description,
    required => required,
});

wire_struct!(rs_market::PlanDependency => pb::PlanDependency {
    id => id,
    version => version,
    installed => installed,
});

impl Wire for rs_market::InstallPlan {
    type Pb = pb::InstallPlan;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::InstallPlan {
            digest,
            summary,
            version,
            place,
            tools,
            egress,
            dependencies,
            secrets,
            models,
            cost,
            licence,
            provenance,
            source_repository,
            version_age_days,
            cannot,
        } = self;
        pb::InstallPlan {
            digest: digest.clone(),
            summary: crate::map::msg(summary),
            version: version.clone(),
            place: buffa::MessageField::some(part_to_pb(place, Some("place"))),
            tools: tools.to_pb(),
            egress: egress.clone(),
            dependencies: dependencies.to_pb(),
            secrets: secrets.to_pb(),
            models: buffa::MessageField::some(part_to_pb(models, Some("models"))),
            cost: buffa::MessageField::some(part_to_pb(cost, Some("cost"))),
            licence: licence.clone(),
            provenance: buffa::MessageField::some(part_to_pb(provenance, Some("provenance"))),
            source_repository: source_repository.clone(),
            version_age_days: version_age_days.unwrap_or_default(),
            cannot: cannot.clone(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            digest: pb.digest,
            summary: crate::map::from_msg(pb.summary, "summary")?,
            version: pb.version,
            place: part_from_pb(&pb.place.into_option().unwrap_or_default(), Some("place"))?,
            tools: Wire::from_pb(pb.tools)?,
            egress: pb.egress,
            dependencies: Wire::from_pb(pb.dependencies)?,
            secrets: Wire::from_pb(pb.secrets)?,
            models: part_from_pb(&pb.models.into_option().unwrap_or_default(), Some("models"))?,
            cost: part_from_pb(&pb.cost.into_option().unwrap_or_default(), Some("cost"))?,
            licence: pb.licence,
            provenance: part_from_pb(
                &pb.provenance.into_option().unwrap_or_default(),
                Some("provenance"),
            )?,
            source_repository: pb.source_repository,
            version_age_days: (pb.version_age_days != 0).then_some(pb.version_age_days),
            cannot: pb.cannot,
        })
    }
}

wire_struct!(rs_market::PlanInstallReply => pb::PlanInstallResponse {
    plan => plan as msg,
});

impl Wire for rs_market::InstallListingParams {
    type Pb = pb::InstallRequest;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::InstallListingParams {
            id,
            version,
            source,
            plan_digest,
            config,
            secrets,
        } = self;
        pb::InstallRequest {
            id: id.clone(),
            version: version.clone().unwrap_or_default(),
            source: source.clone().unwrap_or_default(),
            plan_digest: plan_digest.clone(),
            config: config.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
            secrets: secrets
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            id: pb.id,
            version: none_if_empty(pb.version),
            source: none_if_empty(pb.source),
            plan_digest: pb.plan_digest,
            config: pb.config.into_iter().collect(),
            secrets: pb.secrets.into_iter().collect(),
        })
    }
}

impl Wire for rs_market::InstalledListing {
    type Pb = pb::InstalledListing;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::InstalledListing {
            id,
            version,
            source,
            kind,
            installed_ms,
            revocation,
            update,
        } = self;
        pb::InstalledListing {
            id: id.clone(),
            version: version.clone(),
            source: source.clone(),
            kind: enum_from_str(KIND, kind),
            installed_ms: *installed_ms,
            revocation: crate::map::opt_msg(revocation),
            update: update.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let raw = pb.kind.to_i32();
        Ok(Self {
            id: pb.id,
            version: pb.version,
            source: pb.source,
            kind: str_from_enum(KIND, pb.kind).ok_or_else(|| MapError::enum_value("Kind", raw))?,
            installed_ms: pb.installed_ms,
            revocation: crate::map::from_opt_msg(pb.revocation)?,
            update: none_if_empty(pb.update),
        })
    }
}

wire_struct!(rs_market::InstalledListingReply => pb::InstallResponse {
    installed => installed as msg,
});

/// `UpdateResponse` carries the same field as `InstallResponse`.
pub fn update_response_of(reply: &rs_market::InstalledListingReply) -> pb::UpdateResponse {
    pb::UpdateResponse {
        installed: crate::map::msg(&reply.installed),
        ..Default::default()
    }
}

wire_struct!(rs_market::UninstallListingParams => pb::UninstallRequest {
    id => id,
});

wire_struct!(rs_market::InstalledListingList => pb::ListInstalledResponse {
    installed => installed,
});

wire_struct!(rs_market::AvailableUpdate => pb::AvailableUpdate {
    id => id,
    from => from,
    to => to,
    widens => widens,
});

wire_struct!(rs_market::AvailableUpdateList => pb::CheckUpdatesResponse {
    updates => updates,
});

impl Wire for rs_market::UpdateListingParams {
    type Pb = pb::UpdateRequest;
    fn to_pb(&self) -> Self::Pb {
        let rs_market::UpdateListingParams {
            id,
            version,
            plan_digest,
        } = self;
        pb::UpdateRequest {
            id: id.clone(),
            version: version.clone().unwrap_or_default(),
            plan_digest: plan_digest.clone().unwrap_or_default(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self {
            id: pb.id,
            version: none_if_empty(pb.version),
            plan_digest: none_if_empty(pb.plan_digest),
        })
    }
}

/// `ListSourcesRequest`, `ListInstalledRequest` and `CheckUpdatesRequest`
/// carry nothing: their params are `Empty`.
pub fn empty() -> rs::Empty {
    rs::Empty {}
}
