//! Bounded PE and ECMA-335 metadata reader for the initial compilation profile.

use crate::{
    cil::Reader,
    error::{Error, Result, require},
};
use std::collections::BTreeMap;

#[derive(Clone, Copy)]
enum Column {
    Short,
    Word,
    String,
    Blob,
    Guid,
    Table(u8),
    Coded(u8, &'static [u8]),
}

fn schema(table: u8) -> Result<Vec<Column>> {
    use Column::*;
    let type_ref = Coded(2, &[2, 1, 27]);
    let method_ref = Coded(1, &[6, 10]);
    Ok(match table {
        0 => vec![Short, String, Guid, Guid, Guid],
        1 => vec![Coded(2, &[0, 26, 35, 1]), String, String],
        2 => vec![Word, String, String, type_ref, Table(4), Table(6)],
        3 => vec![Table(4)],
        4 => vec![Short, String, Blob],
        5 => vec![Table(6)],
        6 => vec![Word, Short, Short, String, Blob, Table(8)],
        7 => vec![Table(8)],
        8 => vec![Short, Short, String],
        9 => vec![Table(2), type_ref],
        10 => vec![Coded(3, &[2, 1, 26, 6, 27]), String, Blob],
        11 => vec![Short, Coded(2, &[4, 8, 23]), Blob],
        12 => vec![
            Coded(
                5,
                &[
                    6, 4, 1, 2, 8, 9, 10, 0, 14, 23, 20, 17, 26, 27, 32, 35, 38, 39, 40, 42, 44, 43,
                ],
            ),
            Coded(3, &[6, 10]),
            Blob,
        ],
        13 => vec![Coded(1, &[4, 8]), Blob],
        14 => vec![Short, Coded(2, &[2, 6, 32]), Blob],
        15 => vec![Short, Word, Table(2)],
        16 => vec![Word, Table(4)],
        17 => vec![Blob],
        18 => vec![Table(2), Table(20)],
        19 => vec![Table(20)],
        20 => vec![Short, String, type_ref],
        21 => vec![Table(2), Table(23)],
        22 => vec![Table(23)],
        23 => vec![Short, String, Blob],
        24 => vec![Short, Table(6), Coded(1, &[20, 23])],
        25 => vec![Table(2), method_ref, method_ref],
        26 => vec![String],
        27 => vec![Blob],
        28 => vec![Short, Coded(1, &[4, 6]), String, Table(26)],
        29 => vec![Word, Table(4)],
        30 => vec![Word, Word],
        31 => vec![Word],
        32 => vec![Word, Short, Short, Short, Short, Word, Blob, String, String],
        33 => vec![Word],
        34 => vec![Word, Word, Word],
        35 => vec![Short, Short, Short, Short, Word, Blob, String, String, Blob],
        36 => vec![Word, Table(35)],
        37 => vec![Word, Word, Word, Table(35)],
        38 => vec![Word, String, Blob],
        39 => vec![Word, Word, String, String, Coded(2, &[38, 35, 39])],
        40 => vec![Word, Word, String, Coded(2, &[38, 35, 39])],
        41 => vec![Table(2), Table(2)],
        42 => vec![Short, Short, Coded(1, &[2, 6]), String],
        43 => vec![method_ref, Blob],
        44 => vec![Table(42), type_ref],
        _ => return Err(Error(format!("unsupported metadata table {table}"))),
    })
}

pub(crate) struct Image<'a> {
    pub bytes: &'a [u8],
    pub tables: BTreeMap<u8, Vec<Vec<u32>>>,
    strings: &'a [u8],
    blobs: &'a [u8],
    user_strings: &'a [u8],
    sections: Vec<(u32, u32, u32)>,
}

fn at(bytes: &[u8], offset: usize, size: usize) -> Result<&[u8]> {
    let end = offset
        .checked_add(size)
        .ok_or_else(|| Error("file offset overflow".into()))?;
    bytes
        .get(offset..end)
        .ok_or_else(|| Error("truncated assembly".into()))
}
fn word(bytes: &[u8], offset: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(
        at(bytes, offset, 4)?.try_into().unwrap(),
    ))
}

impl<'a> Image<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<Self> {
        require(at(bytes, 0, 2)? == b"MZ", "missing DOS header")?;
        let pe = word(bytes, 0x3c)? as usize;
        require(at(bytes, pe, 4)? == b"PE\0\0", "missing PE header")?;
        let mut coff = Reader::new(at(bytes, pe + 4, 20)?);
        coff.u16()?;
        let section_count = coff.u16()? as usize;
        coff.take(12)?;
        let optional_size = coff.u16()? as usize;
        let optional = at(bytes, pe + 24, optional_size)?;
        let mut header = Reader::new(optional);
        let directories = match header.u16()? {
            0x10b => 96,
            0x20b => 112,
            _ => return Err(Error("unsupported PE optional header".into())),
        };
        require(
            word(optional, directories - 4)? > 14,
            "missing CLI data directory",
        )?;
        let cli_rva = word(optional, directories + 14 * 8)?;
        let mut sections = Vec::new();
        for index in 0..section_count {
            let section = at(bytes, pe + 24 + optional_size + index * 40, 40)?;
            sections.push((word(section, 12)?, word(section, 16)?, word(section, 20)?));
        }
        let mut image = Self {
            bytes,
            tables: BTreeMap::new(),
            strings: &[],
            blobs: &[],
            user_strings: &[],
            sections,
        };
        let cli = image.rva(cli_rva, 72)?;
        require(word(cli, 0)? >= 72, "invalid CLI header size")?;
        let flags = word(cli, 16)?;
        require(
            flags & 1 != 0 && flags & 0x10 == 0,
            "only managed IL-only assemblies are supported",
        )?;
        let metadata = image.rva(word(cli, 8)?, word(cli, 12)? as usize)?;
        require(
            word(metadata, 0)? == 0x424a5342,
            "invalid CLI metadata signature",
        )?;
        let version_size = word(metadata, 12)? as usize;
        let mut reader = Reader::new(metadata);
        reader.take(16)?;
        reader.take(version_size)?;
        reader.u16()?;
        let stream_count = reader.u16()?;
        let mut streams = BTreeMap::new();
        for _ in 0..stream_count {
            let offset = reader.u32()? as usize;
            let size = reader.u32()? as usize;
            let start = reader.position;
            while reader.u8()? != 0 {
                require(
                    reader.position - start <= 32,
                    "invalid metadata stream name",
                )?;
            }
            let name = std::str::from_utf8(&metadata[start..reader.position - 1])
                .map_err(|_| Error("invalid stream name UTF-8".into()))?
                .to_owned();
            while !reader.position.is_multiple_of(4) {
                reader.u8()?;
            }
            require(
                streams.insert(name, at(metadata, offset, size)?).is_none(),
                "duplicate metadata stream",
            )?;
        }
        image.strings = *streams
            .get("#Strings")
            .ok_or_else(|| Error("missing #Strings".into()))?;
        image.blobs = *streams
            .get("#Blob")
            .ok_or_else(|| Error("missing #Blob".into()))?;
        image.user_strings = streams.get("#US").copied().unwrap_or(&[]);
        let tables = *streams
            .get("#~")
            .ok_or_else(|| Error("only compressed #~ metadata is supported".into()))?;
        image.read_tables(tables)?;
        Ok(image)
    }

    pub fn rva(&self, address: u32, size: usize) -> Result<&'a [u8]> {
        for &(start, raw_size, raw_offset) in &self.sections {
            if let Some(delta) = address.checked_sub(start)
                && delta < raw_size
                && size <= (raw_size - delta) as usize
            {
                return at(self.bytes, raw_offset as usize + delta as usize, size);
            }
        }
        Err(Error(format!("unmapped RVA {address:08x}")))
    }

    pub fn rows(&self, table: u8) -> &[Vec<u32>] {
        self.tables.get(&table).map(Vec::as_slice).unwrap_or(&[])
    }
    pub fn row(&self, table: u8, rid: u32) -> Result<&[u32]> {
        let index = rid
            .checked_sub(1)
            .ok_or_else(|| Error("null metadata row".into()))?;
        self.rows(table)
            .get(index as usize)
            .map(Vec::as_slice)
            .ok_or_else(|| Error(format!("invalid metadata reference {table:02x}:{rid}")))
    }
    pub fn string(&self, index: u32) -> Result<&str> {
        let tail = self
            .strings
            .get(index as usize..)
            .ok_or_else(|| Error("invalid string heap index".into()))?;
        let end = tail
            .iter()
            .position(|b| *b == 0)
            .ok_or_else(|| Error("unterminated metadata string".into()))?;
        std::str::from_utf8(&tail[..end]).map_err(|_| Error("invalid metadata UTF-8".into()))
    }
    pub fn blob(&self, index: u32) -> Result<&[u8]> {
        let mut reader = Reader::new(
            self.blobs
                .get(index as usize..)
                .ok_or_else(|| Error("invalid blob index".into()))?,
        );
        let length = reader.compressed()? as usize;
        reader.take(length)
    }
    pub fn user_string(&self, index: u32) -> Result<Vec<u16>> {
        require(index != 0, "null user string offset")?;
        let mut reader = Reader::new(
            self.user_strings
                .get(index as usize..)
                .ok_or_else(|| Error("invalid user string offset".into()))?,
        );
        let length = reader.compressed()? as usize;
        require(
            length > 0 && length % 2 == 1,
            "invalid user string byte length",
        )?;
        let bytes = reader.take(length)?;
        require(bytes[length - 1] <= 1, "invalid user string terminal flag")?;
        require(
            (length - 1) / 2 <= crate::heap::MAX_STRING_UNITS,
            "string literal exceeds the UTF-16 unit limit",
        )?;
        let units: Vec<_> = bytes[..length - 1]
            .as_chunks::<2>()
            .0
            .iter()
            .copied()
            .map(u16::from_le_bytes)
            .collect();
        Ok(units)
    }
    fn read_tables(&mut self, bytes: &[u8]) -> Result<()> {
        let mut reader = Reader::new(bytes);
        reader.u32()?;
        reader.u8()?;
        reader.u8()?;
        let heaps = reader.u8()?;
        reader.u8()?;
        let valid = u64::from_le_bytes(reader.take(8)?.try_into().unwrap());
        reader.take(8)?;
        let mut counts = [0u32; 64];
        for (table, count) in counts.iter_mut().enumerate() {
            if valid & (1u64 << table) != 0 {
                *count = reader.u32()?;
            }
        }
        for table in 0..64 {
            if counts[table] == 0 {
                continue;
            }
            let columns = schema(table as u8)?;
            let widths: Vec<_> = columns
                .iter()
                .map(|column| match column {
                    Column::Short => 2,
                    Column::Word => 4,
                    Column::String => {
                        if heaps & 1 != 0 {
                            4
                        } else {
                            2
                        }
                    }
                    Column::Guid => {
                        if heaps & 2 != 0 {
                            4
                        } else {
                            2
                        }
                    }
                    Column::Blob => {
                        if heaps & 4 != 0 {
                            4
                        } else {
                            2
                        }
                    }
                    Column::Table(target) => {
                        if counts[*target as usize] >= 65536 {
                            4
                        } else {
                            2
                        }
                    }
                    Column::Coded(bits, targets) => {
                        if targets
                            .iter()
                            .any(|t| counts[*t as usize] >= (1 << (16 - bits)))
                        {
                            4
                        } else {
                            2
                        }
                    }
                })
                .collect();
            let row_size: usize = widths.iter().sum();
            require(
                counts[table] as usize <= (bytes.len() - reader.position) / row_size,
                "truncated metadata table",
            )?;
            let mut rows = Vec::new();
            for _ in 0..counts[table] {
                let mut row = Vec::new();
                for width in &widths {
                    row.push(if *width == 2 {
                        reader.u16()? as u32
                    } else {
                        reader.u32()?
                    });
                }
                rows.push(row);
            }
            self.tables.insert(table as u8, rows);
        }
        Ok(())
    }
}
