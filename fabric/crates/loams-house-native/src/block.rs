//! Data blocks: the protocol's `Block` and the `Native` format's.
//!
//! A block is `VarUInt` columns, `VarUInt` rows, then per column its name, its
//! type name and its data. The protocol adds two things the `Native` format
//! (what chDB writes for `FORMAT Native` and reads for `INSERT … FORMAT Native`,
//! ruling R1.7) does not have:
//!
//! * a `BlockInfo` before the counts, for every revision above 0: field 1
//!   `is_overflows` (`UInt8`), field 2 `bucket_num` (`Int32`), ended by field 0;
//! * from revision 54454, a `has_custom_serialization` byte after each type name
//!   (0 here: the House never sends sparse columns, and at the revisions it
//!   serves a client cannot send them).
//!
//! So a result block from chDB becomes a `Data` packet by adding both, and a
//! client's `INSERT` block becomes `Native` input by removing both: the column
//! data is the same bytes in both (at the revisions served, the `String` offsets
//! layout of 54492 being out of range).

use crate::types::ColumnType;
#[cfg(test)]
use crate::wire::put_u64;
use crate::wire::{Reader, put_bytes, put_i32, put_varuint};
use crate::{Error, revision};

/// How a block is framed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Framing {
    /// A `BlockInfo` precedes the counts.
    pub block_info: bool,
    /// Each type name is followed by its serialization byte.
    pub custom_serialization: bool,
}

impl Framing {
    /// The `Native` format: neither.
    pub const NATIVE_FORMAT: Framing = Framing {
        block_info: false,
        custom_serialization: false,
    };

    /// The protocol's framing at a negotiated revision.
    pub fn protocol(revision: u64) -> Self {
        Self {
            block_info: revision > 0,
            custom_serialization: revision >= revision::CUSTOM_SERIALIZATION,
        }
    }
}

/// `BlockInfo`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockInfo {
    /// Whether the block holds the rows over `max_rows_to_group_by` (`WITH
    /// TOTALS`); always false here.
    pub is_overflows: bool,
    /// The two-level aggregation bucket; -1 for none.
    pub bucket_num: i32,
}

impl Default for BlockInfo {
    fn default() -> Self {
        Self {
            is_overflows: false,
            bucket_num: -1,
        }
    }
}

/// One column of a block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Its name.
    pub name: String,
    /// Its type name, as ClickHouse writes it.
    pub type_name: String,
    /// Its data: serialization prefix and values, exactly as on the wire.
    pub data: Vec<u8>,
}

/// A block.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Block {
    /// Its `BlockInfo` (defaults where the framing has none).
    pub info: BlockInfo,
    /// Its rows.
    pub rows: u64,
    /// Its columns.
    pub columns: Vec<Column>,
}

impl Block {
    /// A block of no columns and no rows: the end of a client's data, and the end
    /// of a result.
    pub fn empty() -> Self {
        Self::default()
    }

    /// A header: the columns' names and types, no rows.
    pub fn header(columns: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            info: BlockInfo::default(),
            rows: 0,
            columns: columns
                .into_iter()
                .map(|(name, type_name)| Column {
                    name,
                    type_name,
                    data: Vec::new(),
                })
                .collect(),
        }
    }

    /// The same columns with no rows.
    pub fn to_header(&self) -> Self {
        Self::header(
            self.columns
                .iter()
                .map(|c| (c.name.clone(), c.type_name.clone())),
        )
    }

    /// Whether this is the empty block (no columns, no rows).
    pub fn is_empty(&self) -> bool {
        self.columns.is_empty() && self.rows == 0
    }

    /// The bytes of its columns' data.
    pub fn data_bytes(&self) -> u64 {
        self.columns.iter().map(|c| c.data.len() as u64).sum()
    }

    /// Reads one block.
    pub fn read(reader: &mut Reader<'_>, framing: Framing) -> Result<Self, Error> {
        let mut info = BlockInfo::default();
        if framing.block_info {
            loop {
                match reader.varuint()? {
                    0 => break,
                    1 => info.is_overflows = reader.bool()?,
                    2 => info.bucket_num = reader.i32()?,
                    other => {
                        return Err(Error::Malformed(format!("BlockInfo field {other}")));
                    }
                }
            }
        }
        let columns = reader.varuint()?;
        let rows = reader.varuint()?;
        if columns > 1 << 20 {
            return Err(Error::Malformed(format!("a block of {columns} columns")));
        }
        let mut out = Vec::with_capacity(columns as usize);
        for _ in 0..columns {
            let name = reader.string()?;
            let type_name = reader.string()?;
            if framing.custom_serialization && reader.u8()? != 0 {
                return Err(Error::Unsupported(format!(
                    "column {name} in a custom serialization (sparse); the House reads only \
                     the default one"
                )));
            }
            let start = reader.position();
            ColumnType::parse(&type_name)?.skip(reader, rows)?;
            out.push(Column {
                name,
                type_name,
                data: reader.since(start).to_vec(),
            });
        }
        Ok(Self {
            info,
            rows,
            columns: out,
        })
    }

    /// Appends the block in `framing`.
    pub fn write(&self, out: &mut Vec<u8>, framing: Framing) {
        if framing.block_info {
            put_varuint(out, 1);
            out.push(u8::from(self.info.is_overflows));
            put_varuint(out, 2);
            put_i32(out, self.info.bucket_num);
            put_varuint(out, 0);
        }
        put_varuint(out, self.columns.len() as u64);
        put_varuint(out, self.rows);
        for column in &self.columns {
            put_bytes(out, column.name.as_bytes());
            put_bytes(out, column.type_name.as_bytes());
            if framing.custom_serialization {
                out.push(0);
            }
            out.extend_from_slice(&column.data);
        }
    }
}

/// The block as a client before revision 54405 reads it: each `LowCardinality(T)`
/// column as `T` (`LowCardinality(Nullable(T))` as `Nullable(T)`), as
/// clickhouse-server converts for such clients. A `LowCardinality` nested in
/// another type is refused.
pub fn without_low_cardinality(mut block: Block) -> Result<Block, Error> {
    for column in &mut block.columns {
        let ty = ColumnType::parse(&column.type_name)?;
        let ColumnType::LowCardinality(inner) = &ty else {
            if ty.has_low_cardinality() {
                return Err(Error::Unsupported(format!(
                    "column {} of type {} for a client of protocol revision before 54405, \
                     which cannot read LowCardinality",
                    column.name, column.type_name
                )));
            }
            continue;
        };
        let inner_text = crate::types::split_type(&column.type_name)
            .and_then(|(_, args)| args.first().map(|a| a.to_string()))
            .ok_or_else(|| Error::Malformed(format!("type {}", column.type_name)))?;
        let (nullable, value) = match inner.as_ref() {
            ColumnType::Nullable(value) => (true, value.as_ref().clone()),
            other => (false, other.clone()),
        };
        column.data = plain_values(&column.data, block.rows, nullable, &value)?;
        column.type_name = inner_text;
    }
    Ok(block)
}

/// A `LowCardinality` column's data as its values (`Nullable`: a null map,
/// then the values).
fn plain_values(
    data: &[u8],
    rows: u64,
    nullable: bool,
    value: &ColumnType,
) -> Result<Vec<u8>, Error> {
    if rows == 0 {
        return Ok(Vec::new());
    }
    let mut reader = Reader::new(data);
    reader.u64()?;
    let mut nulls = Vec::new();
    let mut values = Vec::new();
    let mut dictionary: Vec<&[u8]> = Vec::new();
    let mut done = 0u64;
    while done < rows {
        let index_type = reader.u64()?;
        let width = 1usize << (index_type & 0xff).min(3);
        if index_type & (1 << 9) == 0 {
            return Err(Error::Malformed(
                "a LowCardinality granule with no dictionary of its own".into(),
            ));
        }
        let size = reader.u64()?;
        dictionary.clear();
        for _ in 0..size {
            let start = reader.position();
            value.skip(&mut reader, 1)?;
            dictionary.push(reader.since(start));
        }
        let keys = reader.u64()?;
        if keys == 0 || keys > rows - done {
            return Err(Error::Malformed(format!(
                "a LowCardinality granule of {keys} keys"
            )));
        }
        for _ in 0..keys {
            let raw = reader.take(width)?;
            let mut le = [0u8; 8];
            le[..width].copy_from_slice(raw);
            let key = usize::try_from(u64::from_le_bytes(le)).unwrap_or(usize::MAX);
            let entry = dictionary.get(key).ok_or_else(|| {
                Error::Malformed(format!("a LowCardinality key {key} past its dictionary"))
            })?;
            if nullable {
                nulls.push(u8::from(key == 0));
            }
            values.extend_from_slice(entry);
        }
        done += keys;
    }
    nulls.extend_from_slice(&values);
    Ok(nulls)
}

/// Splits a `Native` format stream (chDB's output) into whole blocks as its
/// bytes arrive.
#[derive(Debug, Default)]
pub struct NativeStream {
    buffer: Vec<u8>,
    /// The buffer's length at the last incomplete attempt: a block is tried again
    /// only once the buffer has grown by half, so a large block arriving in small
    /// pieces is walked a bounded number of times.
    tried_at: usize,
}

impl NativeStream {
    /// An empty stream.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds received bytes.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// The next whole block, if one has arrived.
    pub fn next_block(&mut self) -> Result<Option<Block>, Error> {
        if self.buffer.is_empty() || self.buffer.len() < self.tried_at + self.tried_at / 2 {
            return Ok(None);
        }
        let mut reader = Reader::new(&self.buffer);
        match Block::read(&mut reader, Framing::NATIVE_FORMAT) {
            Ok(block) => {
                let used = reader.position();
                self.buffer.drain(..used);
                self.tried_at = 0;
                Ok(Some(block))
            }
            Err(Error::Incomplete) => {
                self.tried_at = self.buffer.len();
                Ok(None)
            }
            Err(err) => Err(err),
        }
    }

    /// The stream has ended: the blocks still buffered; anything after them is
    /// a partial block.
    pub fn finish(&mut self) -> Result<Vec<Block>, Error> {
        let mut blocks = Vec::new();
        while !self.buffer.is_empty() {
            self.tried_at = 0;
            match self.next_block()? {
                Some(block) => blocks.push(block),
                None => {
                    return Err(Error::Malformed(format!(
                        "the result ended inside a block ({} bytes left)",
                        self.buffer.len()
                    )));
                }
            }
        }
        Ok(blocks)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::tests::hex;

    #[test]
    fn oracle_header_and_rows_reframe() {
        // clickhouse-server 26.9.2.8, revision 54460, `SELECT 1`: the header,
        // then the row.
        let header = hex("010002ffffffff00010001310555496e743800");
        let rows = hex("010002ffffffff00010101310555496e74380001");
        let framing = Framing::protocol(54460);
        let block = Block::read(&mut Reader::new(&rows), framing).expect("reads");
        assert_eq!(block.rows, 1);
        assert_eq!(block.columns[0].name, "1");
        assert_eq!(block.columns[0].data, [1]);
        let mut out = Vec::new();
        block.write(&mut out, framing);
        assert_eq!(out, rows);
        let mut out = Vec::new();
        block.to_header().write(&mut out, framing);
        assert_eq!(out, header);
        // chDB's `FORMAT Native` of the same: no BlockInfo, no serialization byte.
        let mut native = Vec::new();
        block.write(&mut native, Framing::NATIVE_FORMAT);
        assert_eq!(native, hex("010101310555496e743801"));
    }

    #[test]
    fn v1_insert_block_becomes_native_input() {
        // clickhouse-go v1 (revision 54213), golang-migrate's
        // `INSERT INTO schema_migrations (version, dirty, sequence)`.
        let mut wire = hex("010002ffffffff00");
        wire.extend(hex("0301"));
        for (name, ty, data) in [
            ("version", "Int64", "0100000000000000"),
            ("dirty", "UInt8", "01"),
            ("sequence", "UInt64", "37340922b7bae018"),
        ] {
            put_bytes(&mut wire, name.as_bytes());
            put_bytes(&mut wire, ty.as_bytes());
            wire.extend(hex(data));
        }
        let block = Block::read(&mut Reader::new(&wire), Framing::protocol(54213)).expect("reads");
        assert_eq!(block.rows, 1);
        let mut native = Vec::new();
        block.write(&mut native, Framing::NATIVE_FORMAT);
        assert_eq!(native, wire[8..].to_vec());
    }

    #[test]
    fn low_cardinality_for_old_clients() {
        // LowCardinality(Nullable(String)) of ['x', NULL, 'x']: dictionary NULL
        // placeholder, default, 'x'.
        let mut data = Vec::new();
        put_u64(&mut data, 1);
        put_u64(&mut data, 0x600);
        put_u64(&mut data, 3);
        data.extend_from_slice(&[0, 0, 1, b'x']);
        put_u64(&mut data, 3);
        data.extend_from_slice(&[2, 0, 2]);
        let block = Block {
            info: BlockInfo::default(),
            rows: 3,
            columns: vec![Column {
                name: "c".into(),
                type_name: "LowCardinality(Nullable(String))".into(),
                data,
            }],
        };
        let plain = without_low_cardinality(block).expect("converts");
        assert_eq!(plain.columns[0].type_name, "Nullable(String)");
        assert_eq!(plain.columns[0].data, [0, 1, 0, 1, b'x', 0, 1, b'x']);
        let nested =
            Block::header([("a".to_string(), "Array(LowCardinality(String))".to_string())]);
        assert!(without_low_cardinality(nested).is_err());
    }

    #[test]
    fn native_stream_cuts_blocks_from_any_pieces() {
        let mut one = Block::header([("n".to_string(), "UInt64".to_string())]);
        one.rows = 2;
        one.columns[0].data = [1u64.to_le_bytes(), 2u64.to_le_bytes()].concat();
        let mut bytes = Vec::new();
        one.write(&mut bytes, Framing::NATIVE_FORMAT);
        one.write(&mut bytes, Framing::NATIVE_FORMAT);
        let mut stream = NativeStream::new();
        let mut blocks = Vec::new();
        for piece in bytes.chunks(3) {
            stream.push(piece);
            while let Some(block) = stream.next_block().expect("frames") {
                blocks.push(block);
            }
        }
        blocks.extend(stream.finish().expect("ends cleanly"));
        assert_eq!(blocks, vec![one.clone(), one]);
        let mut cut = NativeStream::new();
        cut.push(&bytes[..bytes.len() - 1]);
        assert!(cut.next_block().expect("first").is_some());
        assert!(cut.finish().is_err());
    }
}
