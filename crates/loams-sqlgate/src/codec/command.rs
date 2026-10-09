//! The command phase: classification by first byte, and OK and ERR packets.
//!
//! The gate never parses SQL (D731): it reads only a command's first byte.

use super::handshake::Capabilities;
use super::{DecodeError, Reader, invalid, put_lenenc, put_lenenc_bytes};

/// A client command, by its first byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `COM_QUIT` (0x01).
    Quit,
    /// `COM_SHUTDOWN` (0x08).
    Shutdown,
    /// `COM_DEBUG` (0x0D).
    Debug,
    /// `COM_PING` (0x0E).
    Ping,
    /// `COM_CHANGE_USER` (0x11).
    ChangeUser,
    /// `COM_BINLOG_DUMP` (0x12).
    BinlogDump,
    /// `COM_REGISTER_SLAVE` (0x15).
    RegisterSlave,
    /// `COM_BINLOG_DUMP_GTID` (0x1E).
    BinlogDumpGtid,
    /// Any other command, relayed.
    Other(u8),
}

/// Classifies a command payload by its first byte; `None` if empty.
pub fn classify(payload: &[u8]) -> Option<Command> {
    Some(match *payload.first()? {
        0x01 => Command::Quit,
        0x08 => Command::Shutdown,
        0x0d => Command::Debug,
        0x0e => Command::Ping,
        0x11 => Command::ChangeUser,
        0x12 => Command::BinlogDump,
        0x15 => Command::RegisterSlave,
        0x1e => Command::BinlogDumpGtid,
        b => Command::Other(b),
    })
}

/// The commands TiDB v8.5.8 dispatches (`server/conn.go`), less those the
/// gate refuses by name: `COM_QUIT`, `COM_INIT_DB`, `COM_QUERY`,
/// `COM_FIELD_LIST`, `COM_REFRESH`, `COM_STATISTICS`, `COM_PING`,
/// `COM_STMT_PREPARE`, `COM_STMT_EXECUTE`, `COM_STMT_SEND_LONG_DATA`,
/// `COM_STMT_CLOSE`, `COM_STMT_RESET`, `COM_SET_OPTION`, `COM_STMT_FETCH`
/// and `COM_RESET_CONNECTION`. Everything else (`COM_SLEEP`,
/// `COM_PROCESS_KILL`, `COM_CREATE_DB`, ...) gets 1047 (fix round 1, M2).
pub const RELAYED: [u8; 15] = [
    0x01, 0x02, 0x03, 0x04, 0x07, 0x09, 0x0e, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1f,
];

impl Command {
    /// The error the gate answers with instead of relaying, if refused:
    /// `COM_CHANGE_USER`, replication commands, `COM_SHUTDOWN` and
    /// `COM_DEBUG` get 1235; a command TiDB does not dispatch gets 1047.
    pub fn refusal(self) -> Option<ErrPacket> {
        let what = match self {
            Command::ChangeUser => "COM_CHANGE_USER",
            Command::BinlogDump | Command::BinlogDumpGtid => "binlog dump",
            Command::RegisterSlave => "COM_REGISTER_SLAVE",
            Command::Shutdown => "COM_SHUTDOWN",
            Command::Debug => "COM_DEBUG",
            Command::Quit | Command::Ping => return None,
            Command::Other(b) if RELAYED.contains(&b) => return None,
            Command::Other(_) => {
                return Some(ErrPacket::new(1047, *b"08S01", "Unknown command"));
            }
        };
        Some(ErrPacket::new(
            1235,
            *b"42000",
            &format!("Loams SQL does not support {what}"),
        ))
    }

    /// Counts toward `ReportActivity` (idle detection): every command but
    /// `COM_PING`.
    pub fn is_activity(self) -> bool {
        self != Command::Ping
    }
}

/// An OK packet (header 0x00).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OkPacket {
    /// Affected rows.
    pub affected_rows: u64,
    /// Last insert id.
    pub last_insert_id: u64,
    /// Server status flags.
    pub status: u16,
    /// Warning count.
    pub warnings: u16,
    /// Human-readable info.
    pub info: Vec<u8>,
}

const SERVER_SESSION_STATE_CHANGED: u16 = 0x4000;
const MAX_INFO: usize = 65_536;

impl OkPacket {
    /// The payload for a connection with `caps` (4.1 layout; with
    /// `CLIENT_SESSION_TRACK` the info is length-encoded).
    pub fn encode(&self, caps: Capabilities) -> Vec<u8> {
        let mut out = vec![0x00];
        put_lenenc(&mut out, self.affected_rows);
        put_lenenc(&mut out, self.last_insert_id);
        out.extend_from_slice(&(self.status & !SERVER_SESSION_STATE_CHANGED).to_le_bytes());
        out.extend_from_slice(&self.warnings.to_le_bytes());
        if caps.contains(Capabilities::SESSION_TRACK) {
            if !self.info.is_empty() {
                put_lenenc_bytes(&mut out, &self.info);
            }
        } else {
            out.extend_from_slice(&self.info);
        }
        out
    }

    /// Decodes an OK (0x00) payload. Session-state changes, when flagged,
    /// are skipped after a bounds check.
    pub fn decode(payload: &[u8], caps: Capabilities) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        if r.u8("ok header")? != 0x00 {
            return Err(invalid("ok header", "not 0x00"));
        }
        let affected_rows = r.lenenc("affected rows")?;
        let last_insert_id = r.lenenc("last insert id")?;
        let status = r.u16("status")?;
        let warnings = r.u16("warnings")?;
        let info = if caps.contains(Capabilities::SESSION_TRACK) {
            let info = if r.is_empty() {
                &[][..]
            } else {
                r.lenenc_bytes(MAX_INFO, "info")?
            };
            if status & SERVER_SESSION_STATE_CHANGED != 0 {
                r.lenenc_bytes(MAX_INFO, "session state")?;
            }
            if !r.is_empty() {
                return Err(invalid("ok packet", "trailing bytes"));
            }
            info
        } else {
            let rest = r.rest();
            if rest.len() > MAX_INFO {
                return Err(DecodeError::TooLong {
                    what: "info",
                    limit: MAX_INFO,
                });
            }
            rest
        };
        Ok(Self {
            affected_rows,
            last_insert_id,
            status,
            warnings,
            info: info.to_vec(),
        })
    }
}

/// An ERR packet (header 0xFF, 4.1 layout with SQL state).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrPacket {
    /// The error code.
    pub code: u16,
    /// The five-character SQL state.
    pub sql_state: [u8; 5],
    /// The message.
    pub message: String,
}

impl ErrPacket {
    /// An error.
    pub fn new(code: u16, sql_state: [u8; 5], message: &str) -> Self {
        Self {
            code,
            sql_state,
            message: message.to_owned(),
        }
    }

    /// The payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(9 + self.message.len());
        out.push(0xff);
        out.extend_from_slice(&self.code.to_le_bytes());
        out.push(b'#');
        out.extend_from_slice(&self.sql_state);
        out.extend_from_slice(self.message.as_bytes());
        out
    }

    /// Decodes an ERR payload (message lossily as UTF-8, at most 64 KiB).
    pub fn decode(payload: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(payload);
        if r.u8("err header")? != 0xff {
            return Err(invalid("err header", "not 0xFF"));
        }
        let code = r.u16("error code")?;
        if r.u8("sql state marker")? != b'#' {
            return Err(invalid("sql state marker", "not '#'"));
        }
        let mut sql_state = [0u8; 5];
        sql_state.copy_from_slice(r.bytes(5, "sql state")?);
        let rest = r.rest();
        if rest.len() > MAX_INFO {
            return Err(DecodeError::TooLong {
                what: "error message",
                limit: MAX_INFO,
            });
        }
        Ok(Self {
            code,
            sql_state,
            message: String::from_utf8_lossy(rest).into_owned(),
        })
    }
}

/// How far [`starts_with_kill`] looks into a statement.
pub const KILL_SCAN: usize = 4096;

/// Whether a `COM_QUERY` or `COM_STMT_PREPARE` statement (its first bytes,
/// up to [`KILL_SCAN`]) is a `KILL`: after whitespace, comments and the
/// openers of executable comments (`/*!50700`, `/*T![ttl]`), the first word is
/// `KILL`. A prefix that ends before the first word is treated as a `KILL`
/// (refused, never guessed). The gate refuses `KILL` because a client's
/// connection id (the greeting's) is not TiDB's (fix round 1, M3). This is
/// a usability guard, not a security control: a `KILL` after `;` in a
/// multi-statement query, or inside `PREPARE ... FROM`, is not seen.
/// Enforcement belongs in TiDB (plan R4.3).
pub fn starts_with_kill(sql: &[u8], complete: bool) -> bool {
    let mut at = 0;
    loop {
        while at < sql.len() && sql[at].is_ascii_whitespace() {
            at += 1;
        }
        let rest = &sql[at..];
        if rest.starts_with(b"/*!") {
            // MySQL's executable comment: `/*!` and an optional version.
            at += 3;
            while at < sql.len() && sql[at].is_ascii_digit() {
                at += 1;
            }
        } else if rest.starts_with(b"/*T!") {
            // TiDB's: `/*T!` and an optional `[feature,...]`.
            at += 4;
            if sql.get(at) == Some(&b'[') {
                match sql[at..].iter().position(|&c| c == b']') {
                    Some(end) => at += end + 1,
                    None => return !complete,
                }
            }
        } else if rest.starts_with(b"/*") {
            match rest[2..].windows(2).position(|w| w == b"*/") {
                Some(end) => at += 2 + end + 2,
                None => return !complete,
            }
        } else if rest.starts_with(b"#")
            || (rest.starts_with(b"--") && rest.get(2).is_none_or(|c| c.is_ascii_whitespace()))
        {
            match rest.iter().position(|&c| c == b'\n') {
                Some(end) => at += end + 1,
                None => return !complete,
            }
        } else if rest.starts_with(b"*/") {
            // The end of an executable comment opened before.
            at += 2;
        } else if rest.is_empty() {
            return !complete;
        } else {
            let word = rest.len().min(5);
            if word < 5 && !complete && b"kill".len() >= word {
                // Too short to tell.
                return rest.eq_ignore_ascii_case(&b"kill"[..word]);
            }
            return rest.len() >= 4
                && rest[..4].eq_ignore_ascii_case(b"kill")
                && rest
                    .get(4)
                    .is_none_or(|c| !(c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$'));
        }
    }
}
