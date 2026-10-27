//! The integrity rules of the host report (platform doc 06 PD64, §3.3;
//! design §63 §19): per-boot epochs and keys, the report MAC, and the
//! cumulative-ack rules that never cover a gap silently.
//!
//! [`Sessions`] is sans-I/O: the socket server feeds it what arrived and
//! acts on the [`Verdict`].

use std::collections::BTreeMap;

use hmac::{Hmac, KeyInit, Mac};
use prost::Message;
use rand::Rng;
use sha2::Sha256;

use super::proto::{Gap, HostReport, Invocation};

/// Epoch length in bytes.
pub const EPOCH_LEN: usize = 16;
/// Key length in bytes.
pub const KEY_LEN: usize = 32;

/// The canonical bytes of `invocations`: each one's protobuf encoding,
/// length-delimited, in order. prost writes fields in tag order, so equal
/// invocations always encode equally.
pub fn canonical(invocations: &[Invocation]) -> Vec<u8> {
    let mut out = Vec::new();
    for inv in invocations {
        out.extend_from_slice(&inv.encode_length_delimited_to_vec());
    }
    out
}

fn mac_for(key: &[u8]) -> Hmac<Sha256> {
    // HMAC takes a key of any length; new_from_slice cannot fail for it.
    match <Hmac<Sha256> as KeyInit>::new_from_slice(key) {
        Ok(mac) => mac,
        Err(_) => unreachable!("HMAC accepts keys of any length"),
    }
}

fn report_mac(
    key: &[u8],
    host_id: &str,
    epoch: &[u8],
    seq: u64,
    invocations: &[Invocation],
) -> Hmac<Sha256> {
    let mut mac = mac_for(key);
    mac.update(&(host_id.len() as u64).to_be_bytes());
    mac.update(host_id.as_bytes());
    mac.update(epoch);
    mac.update(&seq.to_be_bytes());
    mac.update(&canonical(invocations));
    mac
}

fn gap_mac(key: &[u8], gap: &Gap) -> Hmac<Sha256> {
    let mut mac = mac_for(key);
    mac.update(b"gap");
    mac.update(&(gap.host_id.len() as u64).to_be_bytes());
    mac.update(gap.host_id.as_bytes());
    mac.update(&gap.epoch);
    mac.update(&gap.from_seq.to_be_bytes());
    mac.update(&gap.to_seq.to_be_bytes());
    mac.update(gap.reason.as_bytes());
    mac
}

/// Signs `report` in place with `key` (the host side).
pub fn sign_report(key: &[u8], report: &mut HostReport) {
    let mac = report_mac(
        key,
        &report.host_id,
        &report.epoch,
        report.seq,
        &report.invocations,
    );
    report.mac = mac.finalize().into_bytes().to_vec();
}

/// Signs `gap` in place with `key` (the host side).
pub fn sign_gap(key: &[u8], gap: &mut Gap) {
    gap.mac = gap_mac(key, gap).finalize().into_bytes().to_vec();
}

/// Whether `report`'s MAC is right for `key` (constant time).
pub fn verify_report(key: &[u8], report: &HostReport) -> bool {
    report_mac(
        key,
        &report.host_id,
        &report.epoch,
        report.seq,
        &report.invocations,
    )
    .verify_slice(&report.mac)
    .is_ok()
}

/// Whether `gap`'s MAC is right for `key` (constant time).
pub fn verify_gap(key: &[u8], gap: &Gap) -> bool {
    gap_mac(key, gap).verify_slice(&gap.mac).is_ok()
}

/// One host process's session with this agent boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The epoch issued to it.
    pub epoch: [u8; EPOCH_LEN],
    /// Its HMAC key.
    pub key: [u8; KEY_LEN],
    /// Every `seq` up to this one is durably recorded (or a recorded gap).
    pub acked: u64,
    /// The uid the host connected with (`SO_PEERCRED`).
    pub uid: u32,
}

/// Why a message was refused. Each is counted by reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rejection {
    /// No session for the host, or its epoch is not the one issued.
    UnknownEpoch,
    /// The MAC does not verify.
    BadMac,
    /// The `seq` skips past the ack without a `Gap` first.
    SeqGap,
    /// A `Gap` that does not start right after the ack, or is backwards.
    BadGap,
    /// Another uid holds this `host_id`.
    PeerMismatch,
}

impl Rejection {
    /// The reason's name, for the `Reject` message and metrics.
    pub fn as_str(self) -> &'static str {
        match self {
            Rejection::UnknownEpoch => "unknown_epoch",
            Rejection::BadMac => "bad_mac",
            Rejection::SeqGap => "seq_gap",
            Rejection::BadGap => "bad_gap",
            Rejection::PeerMismatch => "peer_mismatch",
        }
    }
}

/// What to do with a message that arrived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// New: record it durably, then [`Sessions::acked`] and ack its `seq`.
    Record,
    /// Already recorded (a resend or replay): ack the session's current
    /// `acked` again and record nothing.
    Duplicate,
    /// Refused: count it, send `Reject` and close.
    Reject(Rejection),
}

/// The sessions of this agent boot, by `host_id`.
#[derive(Debug, Default)]
pub struct Sessions {
    sessions: BTreeMap<String, Session>,
    rejected: BTreeMap<&'static str, u64>,
}

impl Sessions {
    /// No sessions.
    pub fn new() -> Self {
        Self::default()
    }

    /// The handshake. A host that presents the epoch this boot issued to it,
    /// from the same uid, resumes its session; any other host (new, or
    /// holding an epoch from an earlier agent boot) gets a fresh epoch and
    /// key, with `acked` at the last ack the host says it received
    /// (`host_acked`): a host can only skip its own reports that way, never
    /// add any. Returns the session.
    pub fn hello(
        &mut self,
        host_id: &str,
        presented_epoch: &[u8],
        uid: u32,
        host_acked: u64,
    ) -> Result<&Session, Rejection> {
        if let Some(existing) = self.sessions.get(host_id) {
            if existing.uid != uid {
                self.count(Rejection::PeerMismatch);
                return Err(Rejection::PeerMismatch);
            }
            if presented_epoch == existing.epoch.as_slice() {
                return Ok(&self.sessions[host_id]);
            }
        }
        let mut rng = rand::rng();
        let mut epoch = [0u8; EPOCH_LEN];
        let mut key = [0u8; KEY_LEN];
        rng.fill(&mut epoch);
        rng.fill(&mut key);
        self.sessions.insert(
            host_id.to_string(),
            Session {
                epoch,
                key,
                acked: host_acked,
                uid,
            },
        );
        Ok(&self.sessions[host_id])
    }

    /// The session of `host_id`.
    pub fn get(&self, host_id: &str) -> Option<&Session> {
        self.sessions.get(host_id)
    }

    /// Checks a report against its session.
    pub fn check_report(&mut self, report: &HostReport) -> Verdict {
        let verdict = match self.sessions.get(&report.host_id) {
            None => Verdict::Reject(Rejection::UnknownEpoch),
            Some(s) if report.epoch != s.epoch.as_slice() => {
                Verdict::Reject(Rejection::UnknownEpoch)
            }
            Some(s) if !verify_report(&s.key, report) => Verdict::Reject(Rejection::BadMac),
            Some(s) if report.seq <= s.acked => Verdict::Duplicate,
            Some(s) if report.seq != s.acked + 1 => Verdict::Reject(Rejection::SeqGap),
            Some(_) => Verdict::Record,
        };
        if let Verdict::Reject(r) = verdict {
            self.count(r);
        }
        verdict
    }

    /// Checks a gap against its session. A gap must start right after the
    /// ack, so the cumulative ack never jumps over reports silently.
    pub fn check_gap(&mut self, gap: &Gap) -> Verdict {
        let verdict = match self.sessions.get(&gap.host_id) {
            None => Verdict::Reject(Rejection::UnknownEpoch),
            Some(s) if gap.epoch != s.epoch.as_slice() => Verdict::Reject(Rejection::UnknownEpoch),
            Some(s) if !verify_gap(&s.key, gap) => Verdict::Reject(Rejection::BadMac),
            Some(s) if gap.to_seq <= s.acked => Verdict::Duplicate,
            Some(s) if gap.from_seq != s.acked + 1 || gap.to_seq < gap.from_seq => {
                Verdict::Reject(Rejection::BadGap)
            }
            Some(_) => Verdict::Record,
        };
        if let Verdict::Reject(r) = verdict {
            self.count(r);
        }
        verdict
    }

    /// Records that everything of `host_id` up to `seq` is durable. The ack
    /// never moves backwards.
    pub fn acked(&mut self, host_id: &str, seq: u64) -> u64 {
        match self.sessions.get_mut(host_id) {
            Some(s) => {
                s.acked = s.acked.max(seq);
                s.acked
            }
            None => 0,
        }
    }

    /// Rejections so far, by reason.
    pub fn rejected(&self) -> &BTreeMap<&'static str, u64> {
        &self.rejected
    }

    fn count(&mut self, r: Rejection) {
        *self.rejected.entry(r.as_str()).or_default() += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(s: &Session, host_id: &str, seq: u64) -> HostReport {
        let mut r = HostReport {
            host_id: host_id.into(),
            seq,
            invocations: vec![Invocation {
                org: "org_a".into(),
                namespace: "prod".into(),
                invocation_id: format!("i{seq}"),
                cpu_usec: 100,
                ..Invocation::default()
            }],
            epoch: s.epoch.to_vec(),
            mac: vec![],
        };
        sign_report(&s.key, &mut r);
        r
    }

    #[test]
    fn signed_report_verifies_and_tampering_fails() {
        let mut sessions = Sessions::new();
        let s = sessions.hello("h1", &[], 1000, 0).unwrap().clone();
        let mut r = report(&s, "h1", 1);
        assert_eq!(sessions.check_report(&r), Verdict::Record);
        r.invocations[0].cpu_usec = 1;
        assert_eq!(
            sessions.check_report(&r),
            Verdict::Reject(Rejection::BadMac)
        );
    }

    #[test]
    fn replay_from_another_epoch_is_refused() {
        let mut old_boot = Sessions::new();
        let s = old_boot.hello("h1", &[], 1000, 0).unwrap().clone();
        let r = report(&s, "h1", 1);
        // A new agent boot knows nothing of the old epoch.
        let mut sessions = Sessions::new();
        sessions.hello("h1", &s.epoch, 1000, 0).unwrap();
        assert_eq!(
            sessions.check_report(&r),
            Verdict::Reject(Rejection::UnknownEpoch)
        );
        assert_eq!(sessions.rejected()["unknown_epoch"], 1);
    }

    #[test]
    fn dedupe_key_is_host_seq() {
        let mut sessions = Sessions::new();
        let s = sessions.hello("h1", &[], 1000, 0).unwrap().clone();
        let r1 = report(&s, "h1", 1);
        assert_eq!(sessions.check_report(&r1), Verdict::Record);
        sessions.acked("h1", 1);
        assert_eq!(sessions.check_report(&r1), Verdict::Duplicate);
        // Another host's seq 1 is its own.
        let s2 = sessions.hello("h2", &[], 1000, 0).unwrap().clone();
        assert_eq!(
            sessions.check_report(&report(&s2, "h2", 1)),
            Verdict::Record
        );
    }

    #[test]
    fn ack_never_covers_a_gap_silently() {
        let mut sessions = Sessions::new();
        let s = sessions.hello("h1", &[], 1000, 0).unwrap().clone();
        assert_eq!(sessions.check_report(&report(&s, "h1", 1)), Verdict::Record);
        sessions.acked("h1", 1);
        // seq 4 without a Gap for 2..=3 is refused.
        assert_eq!(
            sessions.check_report(&report(&s, "h1", 4)),
            Verdict::Reject(Rejection::SeqGap)
        );
        let mut gap = Gap {
            host_id: "h1".into(),
            from_seq: 2,
            to_seq: 3,
            reason: "buffer_full".into(),
            epoch: s.epoch.to_vec(),
            mac: vec![],
        };
        sign_gap(&s.key, &mut gap);
        assert_eq!(sessions.check_gap(&gap), Verdict::Record);
        sessions.acked("h1", 3);
        assert_eq!(sessions.check_report(&report(&s, "h1", 4)), Verdict::Record);
        // A gap that does not start right after the ack is refused.
        let mut late = Gap {
            from_seq: 6,
            to_seq: 7,
            ..gap.clone()
        };
        sign_gap(&s.key, &mut late);
        assert_eq!(
            sessions.check_gap(&late),
            Verdict::Reject(Rejection::BadGap)
        );
    }

    #[test]
    fn reconnect_with_issued_epoch_resumes() {
        let mut sessions = Sessions::new();
        let s = sessions.hello("h1", &[], 1000, 0).unwrap().clone();
        sessions.acked("h1", 5);
        let again = sessions.hello("h1", &s.epoch, 1000, 0).unwrap();
        assert_eq!(again.epoch, s.epoch);
        assert_eq!(again.acked, 5);
        // Another uid cannot take the host id over.
        assert_eq!(
            sessions.hello("h1", &s.epoch, 1001, 0).unwrap_err(),
            Rejection::PeerMismatch
        );
        // The ack never moves backwards.
        assert_eq!(sessions.acked("h1", 3), 5);
    }
}
