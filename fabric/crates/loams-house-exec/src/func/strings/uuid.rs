//! `generateUUIDv4` and `toUUID`, `toUUIDOrNull`, `toUUIDOrZero`. A `UUID`
//! is 16 bytes in text order (`FixedSizeBinary(16)`, R21.3). `toUUID` takes
//! 36 characters with dashes at 8, 13, 18 and 23, or 32 hexadecimal digits,
//! in either case (`376 CANNOT_PARSE_UUID` otherwise), and nothing after
//! them (`6`), as measured.

use std::sync::Arc;

use datafusion::arrow::array::{ArrayRef, FixedSizeBinaryBuilder};
use loams_house_sql::Registry;
use loams_house_types::{ChError, ChType};

use crate::func::chudf::{ChFunction, check_arity, illegal_type};

fn cannot_parse_uuid(s: &str) -> ChError {
    ChError {
        code: 376,
        name: "CANNOT_PARSE_UUID",
        message: format!("Cannot parse uuid {s}: Cannot parse UUID from String"),
    }
}

fn hex_bytes(digits: &[u8]) -> Option<[u8; 16]> {
    let mut out = [0u8; 16];
    for (i, pair) in digits.chunks(2).enumerate() {
        let hi = (pair[0] as char).to_digit(16)?;
        let lo = (pair[1] as char).to_digit(16)?;
        out[i] = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// The bytes of the UUID written `s`.
pub(crate) fn parse_uuid(s: &[u8]) -> Result<[u8; 16], ChError> {
    let text = || String::from_utf8_lossy(s).into_owned();
    let (digits, used): (Vec<u8>, usize) = if s.len() >= 36 && s[8] == b'-' {
        if s[13] != b'-' || s[18] != b'-' || s[23] != b'-' {
            return Err(cannot_parse_uuid(&text()));
        }
        (s[..36].iter().copied().filter(|&c| c != b'-').collect(), 36)
    } else if s.len() >= 32 {
        (s[..32].to_vec(), 32)
    } else {
        return Err(cannot_parse_uuid(&text()));
    };
    if digits.len() != 32 {
        return Err(cannot_parse_uuid(&text()));
    }
    let bytes = hex_bytes(&digits).ok_or_else(|| cannot_parse_uuid(&text()))?;
    if s.len() > used {
        return Err(ChError::cannot_parse_text(format!(
            "Cannot parse string '{}' as UUID: syntax error at position {used}",
            text()
        )));
    }
    Ok(bytes)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OnError {
    Throw,
    Null,
    Zero,
}

fn to_uuid(name: &'static str, on_error: OnError) -> ChFunction {
    ChFunction::new(
        name,
        Arc::new(move |args, _| {
            check_arity(name, args.len(), 1, 1)?;
            match &args[0] {
                ChType::String | ChType::FixedString(_) | ChType::Uuid => {}
                t => return Err(illegal_type(name, 0, t)),
            }
            Ok(if on_error == OnError::Null {
                ChType::Nullable(Box::new(ChType::Uuid))
            } else {
                ChType::Uuid
            })
        }),
        Arc::new(move |call| {
            if call.types[0] == ChType::Uuid {
                return call.array(0);
            }
            let mut out = FixedSizeBinaryBuilder::with_capacity(call.rows, 16);
            let b = call.bytes(0)?;
            for row in 0..call.rows {
                let Some(v) = b.get(row) else {
                    out.append_null();
                    continue;
                };
                let bytes = match (parse_uuid(v), on_error) {
                    (Ok(bytes), _) => bytes,
                    (Err(e), OnError::Throw) => return Err(e),
                    (Err(_), OnError::Null) => {
                        out.append_null();
                        continue;
                    }
                    (Err(_), OnError::Zero) => [0u8; 16],
                };
                out.append_value(bytes)
                    .map_err(|e| ChError::logical_error(e.to_string()))?;
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
}

fn generate_uuid_v4() -> ChFunction {
    ChFunction::new(
        "generateUUIDv4",
        Arc::new(|args, _| {
            check_arity("generateUUIDv4", args.len(), 0, 1)?;
            Ok(ChType::Uuid)
        }),
        Arc::new(|call| {
            let mut out = FixedSizeBinaryBuilder::with_capacity(call.rows, 16);
            for _ in 0..call.rows {
                out.append_value(uuid::Uuid::new_v4().as_bytes())
                    .map_err(|e| ChError::logical_error(e.to_string()))?;
            }
            Ok(Arc::new(out.finish()) as ArrayRef)
        }),
    )
    .volatile()
}

pub(super) fn register(r: &mut Registry) {
    generate_uuid_v4().register(r);
    to_uuid("toUUID", OnError::Throw).register(r);
    to_uuid("toUUIDOrNull", OnError::Null).register(r);
    to_uuid("toUUIDOrZero", OnError::Zero).register(r);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn uuids_parse_as_measured() {
        let want = parse_uuid(b"61f0c404-5cb3-11e7-907b-a6006ad3dba0").unwrap();
        assert_eq!(want[0], 0x61);
        assert_eq!(
            parse_uuid(b"61F0C404-5CB3-11E7-907B-A6006AD3DBA0").unwrap(),
            want
        );
        assert_eq!(
            parse_uuid(b"61f0c4045cb311e7907ba6006ad3dba0").unwrap(),
            want
        );
        for bad in [
            &b"{61f0c404-5cb3-11e7-907b-a6006ad3dba0}"[..],
            b"abc",
            b"",
            b"61f0c404-5cb311e7-907b-a6006ad3dba0",
        ] {
            assert_eq!(parse_uuid(bad).unwrap_err().code, 376);
        }
        assert_eq!(
            parse_uuid(b"61f0c4045cb311e7907ba6006ad3dba0f")
                .unwrap_err()
                .code,
            6
        );
    }
}
