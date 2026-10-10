use bytes::Bytes;
use serde::{Deserialize, Serialize};

use crate::error::LogError;

/// One record: an optional key and value, headers, and a timestamp.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Record {
    /// The record's key, or `None` for a keyless record.
    pub key: Option<Bytes>,
    /// The record's payload.
    pub value: Option<Bytes>,
    /// Header keys may repeat, as in Kafka.
    pub headers: Vec<(String, Option<Bytes>)>,
    /// Milliseconds since the epoch. The writer replaces a negative timestamp
    /// with its own clock.
    pub timestamp_ms: i64,
}

/// A record with the offset the log assigned to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OffsetRecord {
    /// The offset the log assigned to `record`.
    pub offset: u64,
    /// The record itself.
    pub record: Record,
}

/// How a WAL chunk or segment encodes its records (design §02 §5, D20).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(into = "u8", try_from = "u8")]
#[repr(u8)]
pub enum Encoding {
    /// Kafka `RecordBatch` v2.
    Kafka = 0,
    /// Arrow IPC record batches. Reserved: readers return
    /// [`LogError::UnsupportedEncoding`].
    Arrow = 1,
}

impl From<Encoding> for u8 {
    fn from(encoding: Encoding) -> u8 {
        encoding as u8
    }
}

impl TryFrom<u8> for Encoding {
    type Error = LogError;

    fn try_from(value: u8) -> Result<Self, LogError> {
        match value {
            0 => Ok(Encoding::Kafka),
            1 => Ok(Encoding::Arrow),
            other => Err(LogError::UnsupportedEncoding(other)),
        }
    }
}

impl Encoding {
    /// Fails with [`LogError::UnsupportedEncoding`] unless this build can
    /// read the encoding.
    pub fn ensure_readable(self) -> Result<(), LogError> {
        match self {
            Encoding::Kafka => Ok(()),
            Encoding::Arrow => Err(LogError::UnsupportedEncoding(self.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_its_repr_as_a_byte() {
        assert_eq!(u8::from(Encoding::Kafka), 0);
        assert_eq!(u8::from(Encoding::Arrow), 1);
    }

    #[test]
    fn every_byte_round_trips_or_is_refused() {
        assert_eq!(Encoding::try_from(0u8).expect("kafka"), Encoding::Kafka);
        assert_eq!(Encoding::try_from(1u8).expect("arrow"), Encoding::Arrow);
        for byte in 2u8..=u8::MAX {
            assert!(
                matches!(
                    Encoding::try_from(byte),
                    Err(LogError::UnsupportedEncoding(b)) if b == byte
                ),
                "{byte} must be refused with its own value"
            );
        }
    }

    #[test]
    fn serde_uses_the_same_byte_as_the_repr() {
        for encoding in [Encoding::Kafka, Encoding::Arrow] {
            let json = serde_json::to_string(&encoding).expect("serialize");
            assert_eq!(json, u8::from(encoding).to_string());
            assert_eq!(
                serde_json::from_str::<Encoding>(&json).expect("deserialize"),
                encoding
            );
        }
        assert!(serde_json::from_str::<Encoding>("2").is_err());
    }

    #[test]
    fn only_kafka_is_readable_in_this_build() {
        assert!(Encoding::Kafka.ensure_readable().is_ok());
        assert!(matches!(
            Encoding::Arrow.ensure_readable(),
            Err(LogError::UnsupportedEncoding(1))
        ));
    }
}
