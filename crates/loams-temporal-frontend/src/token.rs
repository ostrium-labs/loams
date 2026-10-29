//! Task tokens (plan DX1 X1.R4): the gateway's own `TaskToken` message,
//! followed by an HMAC-SHA256 over it with a key of the gateway's key ring.

use bytes::Bytes;
use hmac::{Hmac, KeyInit, Mac};
use loams_temporal_proto::loams::temporal::internal::v1::{TaskKind, TaskToken};
use prost::Message as _;
use sha2::Sha256;
use tonic::Status;

const MAC_LEN: usize = 32;

/// The keys tokens are signed with: the first signs, every one verifies.
#[derive(Clone)]
pub struct TokenKeys {
    keys: Vec<(String, Vec<u8>)>,
}

impl std::fmt::Debug for TokenKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let ids: Vec<&str> = self.keys.iter().map(|(id, _)| id.as_str()).collect();
        f.debug_struct("TokenKeys").field("key_ids", &ids).finish()
    }
}

impl TokenKeys {
    /// A ring of `(key_id, secret)`, current first. Panics on an empty ring
    /// or a secret shorter than 16 bytes.
    pub fn new(keys: Vec<(String, Vec<u8>)>) -> Self {
        assert!(!keys.is_empty(), "the task-token key ring is empty");
        assert!(
            keys.iter().all(|(_, k)| k.len() >= 16),
            "task-token keys are at least 16 bytes"
        );
        TokenKeys { keys }
    }

    fn mac(&self, key: &[u8], body: &[u8]) -> Vec<u8> {
        let mut m =
            <Hmac<Sha256> as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
        m.update(body);
        m.finalize().into_bytes().to_vec()
    }

    /// Signs `token` with the current key.
    pub fn encode(&self, mut token: TaskToken) -> Bytes {
        let (id, key) = &self.keys[0];
        token.version = 1;
        token.key_id = id.clone();
        let mut body = token.encode_to_vec();
        let mac = self.mac(key, &body);
        body.extend_from_slice(&mac);
        Bytes::from(body)
    }

    /// Verifies and decodes a token of `kind`. A token that does not verify,
    /// or is of another kind, is `INVALID_ARGUMENT` (Temporal's answer to a
    /// malformed token).
    pub fn decode(&self, raw: &[u8], kind: TaskKind) -> Result<TaskToken, Status> {
        let bad = || Status::invalid_argument("Error deserializing task token.");
        if raw.len() <= MAC_LEN {
            return Err(bad());
        }
        let (body, mac) = raw.split_at(raw.len() - MAC_LEN);
        let token = TaskToken::decode(body).map_err(|_| bad())?;
        let key = self
            .keys
            .iter()
            .find(|(id, _)| *id == token.key_id)
            .ok_or_else(bad)?;
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(&key.1).expect("any key length");
        m.update(body);
        m.verify_slice(mac).map_err(|_| bad())?;
        if token.version != 1 || token.kind != kind as i32 {
            return Err(bad());
        }
        Ok(token)
    }
}
