//! API keys (`/iam/v2/api-keys`): create, list, delete.

use reqwest::Method;
use serde_json::{Value, json};

use crate::{Client, Error, Secret};

/// A key just created: its secret is shown once, here.
#[derive(Debug)]
pub struct NewKey {
    pub id: String,
    pub secret: Secret,
}

/// A key as listed (no secret).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiKey {
    pub id: String,
    pub display_name: String,
    pub owner: String,
    pub resource: String,
}

/// Creates a key owned by `owner` (a service account `sa-…` or a user
/// `u-…`) for `resource` (a Kafka cluster `lkc-…` or a Schema Registry
/// `lsrc-…`) in `environment`.
pub async fn create(
    c: &Client,
    owner: &str,
    resource: &str,
    environment: &str,
    display_name: &str,
    description: &str,
) -> Result<NewKey, Error> {
    let body = json!({
        "spec": {
            "display_name": display_name,
            "description": description,
            "owner": { "id": owner },
            "resource": { "id": resource, "environment": environment },
        }
    });
    let what = format!("create an API key for {owner} on {resource}");
    let v = c
        .call(Method::POST, "/iam/v2/api-keys", Some(&body), &what)
        .await?;
    let id = v["id"].as_str().unwrap_or_default();
    let secret = v["spec"]["secret"].as_str().unwrap_or_default();
    if id.is_empty() || secret.is_empty() {
        return Err(Error::Decode {
            what,
            message: "no id or secret in the response".into(),
        });
    }
    Ok(NewKey {
        id: id.to_string(),
        secret: Secret::new(secret),
    })
}

/// The keys `owner` holds on `resource`, every page.
pub async fn list(c: &Client, owner: &str, resource: &str) -> Result<Vec<ApiKey>, Error> {
    let what = format!("list the API keys of {owner} on {resource}");
    let mut path =
        format!("/iam/v2/api-keys?spec.owner={owner}&spec.resource={resource}&page_size=100");
    let mut out = Vec::new();
    for _ in 0..1000 {
        let v = c.call(Method::GET, &path, None, &what).await?;
        for k in v["data"].as_array().map(Vec::as_slice).unwrap_or_default() {
            out.push(parse(k));
        }
        match v["metadata"]["next"].as_str() {
            Some(next) if !next.is_empty() => path = next.to_string(),
            _ => return Ok(out),
        }
    }
    Err(Error::Decode {
        what,
        message: "more than 1000 pages".into(),
    })
}

/// Deletes a key. A key that is already gone is not an error.
pub async fn delete(c: &Client, id: &str) -> Result<(), Error> {
    let what = format!("delete API key {id}");
    match c
        .call(
            Method::DELETE,
            &format!("/iam/v2/api-keys/{id}"),
            None,
            &what,
        )
        .await
    {
        Ok(_) | Err(Error::NotFound(_)) => Ok(()),
        Err(e) => Err(e),
    }
}

fn parse(k: &Value) -> ApiKey {
    let s = |v: &Value| v.as_str().unwrap_or_default().to_string();
    ApiKey {
        id: s(&k["id"]),
        display_name: s(&k["spec"]["display_name"]),
        owner: s(&k["spec"]["owner"]["id"]),
        resource: s(&k["spec"]["resource"]["id"]),
    }
}
