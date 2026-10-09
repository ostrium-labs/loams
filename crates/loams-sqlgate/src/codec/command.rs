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

impl Command {
    /// The error the gate answers with instead of relaying, if refused:
    /// `COM_CHANGE_USER`, replication commands, `COM_SHUTDOWN` and
    /// `COM_DEBUG` get 1235.
    pub fn refusal(self) -> Option<ErrPacket> {
        let what = match self {
            Command::ChangeUser => "COM_CHANGE_USER",
            Command::BinlogDump | Command::BinlogDumpGtid => "binlog dump",
            Command::RegisterSlave => "COM_REGISTER_SLAVE",
            Command::Shutdown => "COM_SHUTDOWN",
            Command::Debug => "COM_DEBUG",
            Command::Quit | Command::Ping | Command::Other(_) => return None,
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
