//! Strict PE32/PE32+ image mapping and ECMA-335 tables/signature reader.
use crate::{CheckError, ErrorKind, Result};
use crate::budget::{Budget, MAX_TEXT_BYTES};
use std::collections::HashMap;

pub const MAX_ASSEMBLY_BYTES: usize = 64 * 1024 * 1024;
const MAX_METADATA_BYTES: usize = 16 * 1024 * 1024;
const MAX_TOTAL_ROWS: u64 = 500_000;
const MAX_TABLE_ROWS: u32 = 250_000;
const MAX_SECTIONS: usize = 96;
const MAX_STREAMS: usize = 16;
const MAX_SIGNATURE_DEPTH: usize = 64;

fn malformed(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Malformed, message) }
fn unsupported(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Unsupported, message) }
fn limit(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Limit, message) }
fn range(data: &[u8], start: usize, len: usize) -> Result<&[u8]> {
    data.get(start..start.checked_add(len).ok_or_else(|| malformed("offset overflow"))?)
        .ok_or_else(|| malformed("truncated image or metadata"))
}
pub fn read_u16(bytes: &[u8], pos: usize) -> Result<u16> {
    let b = range(bytes, pos, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}
pub fn read_u32(bytes: &[u8], pos: usize) -> Result<u32> {
    let b = range(bytes, pos, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}
fn read_u64(bytes: &[u8], pos: usize) -> Result<u64> {
    let b = range(bytes, pos, 8)?;
    Ok(u64::from_le_bytes(b.try_into().expect("length checked")))
}
fn align4(value: usize) -> Result<usize> {
    value.checked_add(3).map(|n| n & !3).ok_or_else(|| malformed("alignment overflow"))
}

#[derive(Clone, Copy)]
struct Section { rva: u32, span: u32, raw: u32, raw_size: u32 }
#[derive(Clone, Copy, Default)]
struct Table { rows: u32, offset: usize, row_size: usize }
#[derive(Clone, Copy)]
enum Column { U16, U32, Str, Blob, Guid, Index(u8), CodedIndex(Coded) }
#[derive(Clone, Copy)]
enum Coded {
    TypeDefOrRef, ResolutionScope, MemberRefParent, HasConstant, HasCustomAttribute,
    CustomAttributeType, HasFieldMarshal, HasDeclSecurity, HasSemantics, MethodDefOrRef,
    MemberForwarded, Implementation, TypeOrMethodDef,
}
impl Coded {
    fn descriptor(self) -> (u32, &'static [u8]) {
        use Coded::*;
        match self {
            TypeDefOrRef => (2, &[2, 1, 27]),
            ResolutionScope => (2, &[0, 26, 35, 1]),
            MemberRefParent => (3, &[2, 1, 26, 6, 27]),
            HasConstant => (2, &[4, 8, 23]),
            HasCustomAttribute => (5, &[6, 4, 1, 2, 8, 9, 10, 0, 14, 23, 20, 17, 26, 27, 32, 35, 38, 39, 40, 42, 44, 43]),
            // Only tags 2 and 3 are valid for CustomAttributeType.
            CustomAttributeType => (3, &[0, 0, 6, 10]),
            HasFieldMarshal => (1, &[4, 8]),
            HasDeclSecurity => (2, &[2, 6, 32]),
            HasSemantics => (1, &[20, 23]),
            MethodDefOrRef => (1, &[6, 10]),
            MemberForwarded => (1, &[4, 6]),
            Implementation => (2, &[38, 35, 39]),
            TypeOrMethodDef => (1, &[2, 6]),
        }
    }
}
fn columns(table: u8) -> Result<&'static [Column]> {
    use Column::*;
    use Coded::*;
    Ok(match table {
        0 => &[U16, Str, Guid, Guid, Guid],
        1 => &[CodedIndex(ResolutionScope), Str, Str],
        2 => &[U32, Str, Str, CodedIndex(TypeDefOrRef), Index(4), Index(6)],
        3 => &[Index(4)],
        4 => &[U16, Str, Blob],
        5 => &[Index(6)],
        6 => &[U32, U16, U16, Str, Blob, Index(8)],
        7 => &[Index(8)],
        8 => &[U16, U16, Str],
        9 => &[Index(2), CodedIndex(TypeDefOrRef)],
        10 => &[CodedIndex(MemberRefParent), Str, Blob],
        11 => &[U16, CodedIndex(HasConstant), Blob],
        12 => &[CodedIndex(HasCustomAttribute), CodedIndex(CustomAttributeType), Blob],
        13 => &[CodedIndex(HasFieldMarshal), Blob],
        14 => &[U16, CodedIndex(HasDeclSecurity), Blob],
        15 => &[U16, U32, Index(2)],
        16 => &[U32, Index(4)],
        17 => &[Blob],
        18 => &[Index(2), Index(20)],
        19 => &[Index(20)],
        20 => &[U16, Str, CodedIndex(TypeDefOrRef)],
        21 => &[Index(2), Index(23)],
        22 => &[Index(23)],
        23 => &[U16, Str, Blob],
        24 => &[U16, Index(6), CodedIndex(HasSemantics)],
        25 => &[Index(2), CodedIndex(MethodDefOrRef), CodedIndex(MethodDefOrRef)],
        26 => &[Str],
        27 => &[Blob],
        28 => &[U16, CodedIndex(MemberForwarded), Str, Index(26)],
        29 => &[U32, Index(4)],
        30 => &[U32, U32],
        31 => &[U32],
        32 => &[U32, U16, U16, U16, U16, U32, Blob, Str, Str],
        33 => &[U32],
        34 => &[U32, U32, U32],
        35 => &[U16, U16, U16, U16, U32, Blob, Str, Str, Blob],
        36 => &[U32, Index(35)],
        37 => &[U32, U32, U32, Index(35)],
        38 => &[U32, Str, Blob],
        39 => &[U32, U32, Str, Str, CodedIndex(Implementation)],
        40 => &[U32, U32, Str, CodedIndex(Implementation)],
        41 => &[Index(2), Index(2)],
        42 => &[U16, U16, CodedIndex(TypeOrMethodDef), Str],
        43 => &[CodedIndex(MethodDefOrRef), Blob],
        44 => &[Index(42), CodedIndex(TypeDefOrRef)],
        _ => return Err(unsupported("unknown nonempty ECMA metadata table")),
    })
}

pub(crate) struct Metadata<'a> {
    image: &'a [u8],
    sections: Vec<Section>,
    tables_stream: &'a [u8],
    tables: [Table; 64],
    strings: &'a [u8],
    blobs: &'a [u8],
    user_strings: &'a [u8],
    heap_sizes: u8,
}
impl<'a> Metadata<'a> {
    pub fn parse(image: &'a [u8]) -> Result<Self> {
        if image.len() > MAX_ASSEMBLY_BYTES { return Err(limit("assembly exceeds size cap")); }
        if image.len() < 2 || &image[..2] != b"MZ" {
            return Err(CheckError::new(ErrorKind::NonManaged, "a managed PE assembly was expected"));
        }
        let pe_offset = read_u32(image, 0x3c)? as usize;
        if range(image, pe_offset, 4)? != b"PE\0\0" { return Err(malformed("invalid PE signature")); }
        let sections_count = read_u16(image, pe_offset + 6)? as usize;
        if sections_count == 0 || sections_count > MAX_SECTIONS { return Err(limit("invalid PE section count")); }
        let optional_size = read_u16(image, pe_offset + 20)? as usize;
        let optional = pe_offset.checked_add(24).ok_or_else(|| malformed("PE offset overflow"))?;
        let magic = read_u16(image, optional)?;
        let (dir_count_pos, dirs_pos) = match magic {
            0x10b => (92usize, 96usize),
            0x20b => (108usize, 112usize),
            _ => return Err(unsupported("not a PE32 or PE32+ image")),
        };
        if optional_size < dirs_pos { return Err(malformed("truncated PE optional header")); }
        let dir_count = read_u32(image, optional + dir_count_pos)?;
        if dir_count <= 14 { return Err(CheckError::new(ErrorKind::NonManaged, "PE has no CLI data directory")); }
        if optional_size < dirs_pos + 15 * 8 { return Err(malformed("CLI data directory is truncated")); }
        let cli_dir = optional + dirs_pos + 14 * 8;
        let cli_rva = read_u32(image, cli_dir)?;
        let cli_size = read_u32(image, cli_dir + 4)?;
        if cli_rva == 0 || cli_size == 0 { return Err(CheckError::new(ErrorKind::NonManaged, "PE has no CLI header")); }
        let section_base = optional.checked_add(optional_size).ok_or_else(|| malformed("section offset overflow"))?;
        range(image, section_base, sections_count.checked_mul(40).ok_or_else(|| malformed("section count overflow"))?)?;
        let mut sections = Vec::with_capacity(sections_count);
        for i in 0..sections_count {
            let p = section_base + i * 40;
            let vsize = read_u32(image, p + 8)?;
            let rva = read_u32(image, p + 12)?;
            let raw_size = read_u32(image, p + 16)?;
            let raw = read_u32(image, p + 20)?;
            if raw_size > 0 { range(image, raw as usize, raw_size as usize)?; }
            let span = vsize.max(raw_size);
            if rva.checked_add(span).is_none() { return Err(malformed("PE section RVA overflow")); }
            if sections.iter().any(|prior: &Section| span != 0 && prior.span != 0 &&
                rva < prior.rva + prior.span && prior.rva < rva + span) {
                return Err(malformed("overlapping PE section RVA ranges"));
            }
            sections.push(Section { rva, span, raw, raw_size });
        }
        let mut reader = Self { image, sections, tables_stream: &[], tables: [Table::default(); 64],
            strings: &[], blobs: &[], user_strings: &[], heap_sizes: 0 };
        if cli_size < 0x48 { return Err(malformed("CLI header is truncated")); }
        let cli = reader.rva_range(cli_rva, 0x48)?;
        let md_rva = read_u32(cli, 8)?;
        let md_size = read_u32(cli, 12)? as usize;
        if md_rva == 0 || md_size == 0 { return Err(malformed("CLI metadata directory is missing")); }
        if md_size > MAX_METADATA_BYTES { return Err(limit("metadata directory exceeds size cap")); }
        let root = reader.rva_range(md_rva, md_size)?;
        if read_u32(root, 0)? != 0x424a_5342 { return Err(malformed("invalid BSJB metadata signature")); }
        let version_len = read_u32(root, 12)? as usize;
        if version_len > 256 { return Err(limit("metadata version string too long")); }
        let mut cursor = align4(16usize.checked_add(version_len).ok_or_else(|| malformed("metadata overflow"))?)?;
        range(root, 16, version_len)?;
        let streams_count = read_u16(root, cursor + 2)? as usize;
        cursor += 4;
        if streams_count == 0 || streams_count > MAX_STREAMS { return Err(limit("metadata stream count out of range")); }
        let mut streams: HashMap<&str, &'a [u8]> = HashMap::new();
        let mut occupied: Vec<(usize, usize)> = Vec::new();
        for _ in 0..streams_count {
            let offset = read_u32(root, cursor)? as usize;
            let size = read_u32(root, cursor + 4)? as usize;
            cursor += 8;
            let name_start = cursor;
            loop {
                let b = *range(root, cursor, 1)?.first().ok_or_else(|| malformed("missing stream terminator"))?;
                cursor += 1;
                if b == 0 { break; }
                if cursor - name_start > 32 { return Err(limit("stream name exceeds cap")); }
            }
            let name = std::str::from_utf8(range(root, name_start, cursor - name_start - 1)?)
                .map_err(|_| malformed("stream name is not UTF-8"))?;
            cursor = align4(cursor)?;
            let end = offset.checked_add(size).ok_or_else(|| malformed("metadata stream overflow"))?;
            let stream = range(root, offset, size)?;
            if occupied.iter().any(|&(a, b)| size != 0 && offset < b && a < end) {
                return Err(malformed("overlapping metadata streams"));
            }
            occupied.push((offset, end));
            if streams.insert(name, stream).is_some() { return Err(malformed("duplicate metadata stream")); }
        }
        if occupied.iter().any(|&(start, end)| start < cursor && end > start) {
            return Err(malformed("metadata stream overlaps stream directory"));
        }
        let tables = streams.get("#~").or_else(|| streams.get("#-"))
            .copied().ok_or_else(|| malformed("missing metadata tables stream"))?;
        reader.strings = streams.get("#Strings").copied().ok_or_else(|| malformed("missing #Strings"))?;
        reader.blobs = streams.get("#Blob").copied().ok_or_else(|| malformed("missing #Blob"))?;
        reader.user_strings = streams.get("#US").copied().unwrap_or(&[]);
        reader.tables_stream = tables;
        reader.read_tables()?;
        Ok(reader)
    }
    pub fn rva_tail(&self, rva: u32) -> Result<&'a [u8]> {
        for s in &self.sections {
            if let Some(relative) = rva.checked_sub(s.rva) {
                if relative < s.span {
                    if relative >= s.raw_size { return Err(malformed("RVA points into unbacked PE section")); }
                    let start = (s.raw as usize).checked_add(relative as usize)
                        .ok_or_else(|| malformed("section file offset overflow"))?;
                    let size = (s.raw_size - relative) as usize;
                    return range(self.image, start, size);
                }
            }
        }
        Err(malformed("RVA is outside all PE sections"))
    }
    fn rva_range(&self, rva: u32, size: usize) -> Result<&'a [u8]> {
        let tail = self.rva_tail(rva)?;
        range(tail, 0, size)
    }
    fn read_tables(&mut self) -> Result<()> {
        let bytes = self.tables_stream;
        range(bytes, 0, 24)?;
        self.heap_sizes = bytes[6];
        if self.heap_sizes & !7 != 0 { return Err(malformed("unknown metadata heap-size flags")); }
        let valid = read_u64(bytes, 8)?;
        if valid >> 45 != 0 { return Err(unsupported("reserved metadata tables are populated")); }
        let mut pos = 24usize;
        let mut total = 0u64;
        for id in 0..64 {
            if valid & (1u64 << id) == 0 { continue; }
            let count = read_u32(bytes, pos)?;
            pos += 4;
            if count > MAX_TABLE_ROWS { return Err(limit("metadata table exceeds row cap")); }
            total += u64::from(count);
            if total > MAX_TOTAL_ROWS { return Err(limit("metadata row total exceeds cap")); }
            self.tables[id].rows = count;
        }
        if self.tables[3].rows != 0 || self.tables[5].rows != 0 || self.tables[7].rows != 0 {
            return Err(unsupported("unoptimized pointer tables are not supported"));
        }
        let mut offset = pos;
        for id in 0..64 {
            let count = self.tables[id].rows;
            if count == 0 { continue; }
            let width = columns(id as u8)?.iter().try_fold(0usize, |width, &column| {
                width.checked_add(self.column_size(column)).ok_or_else(|| malformed("row width overflow"))
            })?;
            let size = width.checked_mul(count as usize).ok_or_else(|| malformed("metadata table size overflow"))?;
            range(bytes, offset, size)?;
            self.tables[id].row_size = width;
            self.tables[id].offset = offset;
            offset += size;
        }
        Ok(())
    }
    fn column_size(&self, col: Column) -> usize {
        match col {
            Column::U16 => 2, Column::U32 => 4,
            Column::Str if self.heap_sizes & 1 == 0 => 2,
            Column::Guid if self.heap_sizes & 2 == 0 => 2,
            Column::Blob if self.heap_sizes & 4 == 0 => 2,
            Column::Str | Column::Guid | Column::Blob => 4,
            Column::Index(t) => if self.tables[t as usize].rows < 0x10000 { 2 } else { 4 },
            Column::CodedIndex(kind) => {
                let (bits, ids) = kind.descriptor();
                if ids.iter().all(|&id| self.tables[id as usize].rows < (1u32 << (16 - bits))) { 2 } else { 4 }
            }
        }
    }
    pub fn count(&self, table: u8) -> u32 { self.tables[table as usize].rows }
    // All ECMA-335 tables have at most 9 columns. Returning an inline array
    // prevents a fresh heap allocation for every row and keeps bad tables from
    // turning metadata enumeration into unaccounted allocation churn.
    pub fn row(&self, table: u8, index: u32) -> Result<[u32; 9]> {
        if table as usize >= self.tables.len() {
            return Err(malformed("metadata token table index out of range"));
        }
        let t = self.tables[table as usize];
        if index == 0 || index > t.rows { return Err(malformed("metadata row index out of range")); }
        let start = t.offset.checked_add(((index - 1) as usize)
            .checked_mul(t.row_size).ok_or_else(|| malformed("metadata row offset overflow"))?)
            .ok_or_else(|| malformed("metadata row offset overflow"))?;
        let cols = columns(table)?;
        let mut values = [0u32; 9];
        let mut pos = start;
        for (i, &col) in cols.iter().enumerate() {
            let size = self.column_size(col);
            values[i] = match size {
                2 => u32::from(read_u16(self.tables_stream, pos)?),
                4 => read_u32(self.tables_stream, pos)?,
                _ => return Err(malformed("invalid column width")),
            };
            pos += size;
        }
        Ok(values)
    }
    pub fn string(&self, index: u32) -> Result<&'a str> {
        if index == 0 { return Ok(""); }
        let bytes = self.strings.get(index as usize..).ok_or_else(|| malformed("invalid #Strings index"))?;
        let len = bytes.iter().position(|&b| b == 0).ok_or_else(|| malformed("unterminated #Strings value"))?;
        std::str::from_utf8(&bytes[..len]).map_err(|_| malformed("#Strings has invalid UTF-8"))
    }
    pub fn blob(&self, index: u32) -> Result<&'a [u8]> {
        if index == 0 { return Ok(&[]); }
        let bytes = self.blobs.get(index as usize..).ok_or_else(|| malformed("invalid #Blob index"))?;
        let mut sig = SigReader::new(bytes);
        let len = sig.compressed()? as usize;
        range(bytes, sig.pos, len)
    }
    pub fn user_string(&self, index: u32, budget: &Budget) -> Result<String> {
        if index == 0 { return Ok(String::new()); }
        let bytes = self.user_strings.get(index as usize..).ok_or_else(|| malformed("invalid #US index"))?;
        let mut sig = SigReader::new(bytes);
        let len = sig.compressed()? as usize;
        if len == 0 { return Ok(String::new()); }
        let payload = range(bytes, sig.pos, len)?;
        if (len - 1) % 2 != 0 { return Err(malformed("invalid user string UTF-16 length")); }
        let units = || payload[..len - 1].chunks_exact(2)
            .map(|p| u16::from_le_bytes([p[0], p[1]]));
        // Validate and measure before allocating. Never build an intermediate
        // Vec<u16>, even when many ldstr instructions share a large #US item.
        let mut capacity = 0usize;
        for scalar in std::char::decode_utf16(units()) {
            let scalar = scalar.map_err(|_| malformed("invalid #US UTF-16"))?;
            capacity = capacity.checked_add(scalar.len_utf8()).ok_or_else(|| limit("user string length overflow"))?;
            if capacity > MAX_TEXT_BYTES { return Err(limit("user string exceeds text cap")); }
        }
        budget.charge(capacity)?;
        let mut out = String::new();
        out.try_reserve_exact(capacity).map_err(|_| limit("user string allocation failed"))?;
        for scalar in std::char::decode_utf16(units()) {
            out.push(scalar.map_err(|_| malformed("invalid #US UTF-16"))?);
        }
        Ok(out)
    }
}

pub(crate) fn coded_token(kind: &'static str, value: u32) -> Result<u32> {
    let (bits, tables): (u32, &[u8]) = match kind {
        "TypeDefOrRef" => (2, &[2, 1, 27]),
        "ResolutionScope" => (2, &[0, 26, 35, 1]),
        "MemberRefParent" => (3, &[2, 1, 26, 6, 27]),
        "MethodDefOrRef" => (1, &[6, 10]),
        _ => return Err(unsupported("unknown coded reference type")),
    };
    let tag = (value & ((1 << bits) - 1)) as usize;
    let table = *tables.get(tag).ok_or_else(|| malformed("invalid coded index tag"))?;
    let index = value >> bits;
    if index == 0 { return Err(malformed("null metadata entity reference")); }
    Ok((u32::from(table) << 24) | index)
}

pub(crate) struct Names<'m, 'a, 'b> {
    meta: &'m Metadata<'a>,
    budget: &'b Budget,
    resolved: HashMap<u32, String>,
    resolving: Vec<u32>,
    method_owner: Vec<u32>,
    field_owner: Vec<u32>,
    nested: HashMap<u32, u32>,
}
#[derive(Clone)]
pub(crate) struct MethodSignature {
    pub formatted: String,
    pub params: Vec<String>,
    pub return_integral: bool,
    pub params_integral: bool,
}
impl<'m, 'a, 'b> Names<'m, 'a, 'b> {
    pub fn new(meta: &'m Metadata<'a>, budget: &'b Budget) -> Result<Self> {
        let type_count = meta.count(2);
        let methods = meta.count(6) as usize + 1;
        let fields = meta.count(4) as usize + 1;
        if type_count == 0 && (methods > 1 || fields > 1) {
            return Err(malformed("member definitions without declaring types"));
        }
        let mut method_owner = budget.vec::<u32>(methods)?;
        method_owner.resize(methods, 0);
        let mut field_owner = budget.vec::<u32>(fields)?;
        field_owner.resize(fields, 0);
        let mut prev_field = 1u32;
        let mut prev_method = 1u32;
        for i in 1..=type_count {
            let row = meta.row(2, i)?;
            let next = if i < type_count { meta.row(2, i + 1)? } else {
                [0, 0, 0, 0, meta.count(4) + 1, meta.count(6) + 1, 0, 0, 0]
            };
            if row[4] != prev_field || row[5] != prev_method {
                return Err(malformed("type definition member ranges have gaps or overlap"));
            }
            prev_field = next[4];
            prev_method = next[5];
            for (table, start, end, owner) in [
                (6u8, row[5], next[5], &mut method_owner),
                (4u8, row[4], next[4], &mut field_owner),
            ] {
                if start == 0 || start > end || end > meta.count(table) + 1 {
                    return Err(malformed("invalid type member list range"));
                }
                for id in start..end { owner[id as usize] = i; }
            }
        }
        let mut nested = HashMap::new();
        let nested_count = meta.count(41) as usize;
        budget.map_entries::<u32, u32>(nested_count)?;
        nested.try_reserve(nested_count).map_err(|_| limit("nested-type map allocation failed"))?;
        for id in 1..=meta.count(41) {
            let row = meta.row(41, id)?;
            if row[0] == 0 || row[0] > type_count || row[1] == 0 || row[1] > type_count || nested.insert(row[0], row[1]).is_some() {
                return Err(malformed("invalid nested-type metadata"));
            }
        }
        let resolving = budget.vec::<u32>(MAX_SIGNATURE_DEPTH)?;
        Ok(Self { meta, budget, resolved: HashMap::new(), resolving, method_owner, field_owner, nested })
    }
    pub fn entity(&mut self, token: u32) -> Result<String> {
        if let Some(value) = self.resolved.get(&token) { return self.budget.owned(value); }
        if self.resolving.len() >= MAX_SIGNATURE_DEPTH { return Err(limit("metadata entity depth exceeds cap")); }
        if self.resolving.contains(&token) { return Err(malformed("cyclic metadata reference")); }
        self.resolving.push(token);
        let value = self.resolve_entity(token);
        self.resolving.pop();
        if let Ok(value) = &value {
            let cached = self.budget.owned(value)?;
            self.budget.map_entries::<u32, String>(1)?;
            self.resolved.try_reserve(1).map_err(|_| limit("metadata name cache allocation failed"))?;
            self.resolved.insert(token, cached);
        }
        value
    }
    fn resolve_entity(&mut self, token: u32) -> Result<String> {
        let table = (token >> 24) as u8;
        let index = token & 0x00ff_ffff;
        let row = self.meta.row(table, index)?;
        match table {
            0 => self.budget.owned("<module>"),
            1 => {
                let name = self.meta.string(row[1])?;
                let scope = row[0];
                if scope & 3 == 3 {
                    let parent = self.entity(coded_token("ResolutionScope", scope)?)?;
                    self.budget.text(format_args!("{parent}+{name}"))
                } else {
                    let qualified = qualified(self.budget, self.meta.string(row[2])?, name)?;
                    if scope == 0 { Ok(qualified) }
                    else {
                        let prefix = self.entity(coded_token("ResolutionScope", scope)?)?;
                        self.budget.text(format_args!("[{prefix}]{qualified}"))
                    }
                }
            }
            2 => {
                let name = self.meta.string(row[1])?;
                if let Some(&outer) = self.nested.get(&index) {
                    let parent = self.entity((2u32 << 24) | outer)?;
                    self.budget.text(format_args!("{parent}+{name}"))
                } else { qualified(self.budget, self.meta.string(row[2])?, name) }
            }
            4 => {
                let owner = *self.field_owner.get(index as usize).filter(|&&n| n != 0)
                    .ok_or_else(|| malformed("field has no declaring type"))?;
                let name = self.meta.string(row[1])?;
                let field = self.field_signature(row[2])?;
                let parent = self.entity((2u32 << 24) | owner)?;
                self.budget.text(format_args!("{parent}::{name}:{field}"))
            }
            6 => {
                let owner = *self.method_owner.get(index as usize).filter(|&&n| n != 0)
                    .ok_or_else(|| malformed("method has no declaring type"))?;
                let name = self.meta.string(row[3])?;
                let sig = self.method_signature(row[4])?;
                let parent = self.entity((2u32 << 24) | owner)?;
                self.budget.text(format_args!("{parent}::{name}{}", sig.formatted))
            }
            10 => {
                let parent = coded_token("MemberRefParent", row[0])?;
                let name = self.meta.string(row[1])?;
                let sig_blob = self.meta.blob(row[2])?;
                let sig = if sig_blob.first().is_some_and(|c| c & 0x0f == 6) {
                    let field = self.field_signature(row[2])?;
                    self.budget.text(format_args!(":{field}"))?
                } else { self.method_signature(row[2])?.formatted };
                let parent = self.entity(parent)?;
                self.budget.text(format_args!("{parent}::{name}{sig}"))
            }
            17 => {
                let blob = self.meta.blob(row[0])?;
                if blob.first() == Some(&7) {
                    let values = self.locals(row[0])?;
                    let joined = self.budget.join(&values, ", ")?;
                    self.budget.text(format_args!("locals {joined}"))
                } else { Ok(self.method_signature(row[0])?.formatted) }
            }
            26 => self.budget.owned(self.meta.string(row[0])?),
            27 => self.type_spec(row[0]),
            35 => self.budget.owned(self.meta.string(row[6])?),
            43 => {
                let target = coded_token("MethodDefOrRef", row[0])?;
                let args = self.type_args(row[1])?;
                let parent = self.entity(target)?;
                let joined = self.budget.join(&args, ", ")?;
                self.budget.text(format_args!("{parent}<{joined}>"))
            }
            32 => Err(unsupported("assembly definition is not an IL operand entity")),
            _ => Err(malformed("invalid IL metadata token kind")),
        }
    }
    pub fn method_signature(&mut self, blob: u32) -> Result<MethodSignature> {
        let mut parser = SigReader::new(self.meta.blob(blob)?);
        let result = parser.method(self, 0)?;
        parser.finished()?;
        Ok(result)
    }
    pub fn field_signature(&mut self, blob: u32) -> Result<String> {
        let mut parser = SigReader::new(self.meta.blob(blob)?);
        if parser.byte()? != 6 { return Err(malformed("invalid field signature")); }
        let result = parser.ty(self, 0)?;
        parser.finished()?;
        Ok(result)
    }
    pub fn locals(&mut self, index: u32) -> Result<Vec<String>> {
        let mut parser = SigReader::new(self.meta.blob(index)?);
        if parser.byte()? != 7 { return Err(malformed("invalid local signature header")); }
        let count = parser.compressed()? as usize;
        if count > 65_535 { return Err(limit("local signature count exceeds cap")); }
        let mut types = self.budget.vec::<String>(count)?;
        for _ in 0..count { types.push(parser.ty(self, 0)?); }
        parser.finished()?;
        Ok(types)
    }
    fn type_spec(&mut self, blob: u32) -> Result<String> {
        let mut parser = SigReader::new(self.meta.blob(blob)?);
        let result = parser.ty(self, 0)?;
        parser.finished()?;
        Ok(result)
    }
    fn type_args(&mut self, blob: u32) -> Result<Vec<String>> {
        let mut p = SigReader::new(self.meta.blob(blob)?);
        if p.byte()? != 0x0a { return Err(malformed("invalid generic method instantiation signature")); }
        let count = p.compressed()? as usize;
        if count > 256 { return Err(limit("generic argument count exceeds cap")); }
        let mut types = self.budget.vec::<String>(count)?;
        for _ in 0..count { types.push(p.ty(self, 0)?); }
        p.finished()?;
        Ok(types)
    }
    pub fn user_string(&self, index: u32) -> Result<String> { self.meta.user_string(index, self.budget) }
    pub fn quoted(&self, value: &str) -> Result<String> { self.budget.quoted(value) }
    pub fn text(&self, args: std::fmt::Arguments<'_>) -> Result<String> { self.budget.text(args) }
    pub fn join(&self, values: &[String], separator: &str) -> Result<String> { self.budget.join(values, separator) }
    pub fn charge_array<T>(&self, count: usize) -> Result<()> { self.budget.charge_array::<T>(count) }
    pub fn charge(&self, bytes: usize) -> Result<()> { self.budget.charge(bytes) }
    pub fn local_token(&mut self, token: u32) -> Result<Vec<String>> {
        if token >> 24 != 17 { return Err(malformed("invalid local signature token")); }
        let row = self.meta.row(17, token & 0x00ff_ffff)?;
        self.locals(row[0])
    }
}
fn qualified(budget: &Budget, ns: &str, name: &str) -> Result<String> {
    if ns.is_empty() { budget.owned(name) } else { budget.text(format_args!("{ns}.{name}")) }
}

struct SigReader<'a> { bytes: &'a [u8], pos: usize }
impl<'a> SigReader<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, pos: 0 } }
    fn byte(&mut self) -> Result<u8> {
        let result = *self.bytes.get(self.pos).ok_or_else(|| malformed("truncated signature"))?;
        self.pos += 1; Ok(result)
    }
    fn compressed(&mut self) -> Result<u32> {
        let b = self.byte()?;
        if b & 0x80 == 0 { return Ok(u32::from(b)); }
        if b & 0xc0 == 0x80 {
            let value = (u32::from(b & 0x3f) << 8) | u32::from(self.byte()?);
            return if value >= 0x80 { Ok(value) } else { Err(malformed("noncanonical compressed integer")) };
        }
        if b & 0xe0 == 0xc0 {
            let c = self.byte()?; let d = self.byte()?; let e = self.byte()?;
            let value = (u32::from(b & 0x1f) << 24) | (u32::from(c) << 16) | (u32::from(d) << 8) | u32::from(e);
            return if value >= 0x4000 { Ok(value) } else { Err(malformed("noncanonical compressed integer")) };
        }
        Err(malformed("invalid compressed metadata integer"))
    }
    fn finished(&self) -> Result<()> {
        if self.pos == self.bytes.len() { Ok(()) } else { Err(malformed("trailing bytes in metadata signature")) }
    }
    fn signed(&mut self) -> Result<i32> {
        // ECMA-335 II.23.2: signed compression uses the low sign bit,
        // and canonical width must be checked *after sign extension*.
        // Reusing compressed() here would incorrectly reject 80 01 (-8192)
        // and C0 00 00 01 (-268435456): both are valid signed encodings.
        let b = self.byte()?;
        let (raw, bits) = if b & 0x80 == 0 {
            (u32::from(b), 7)
        } else if b & 0xc0 == 0x80 {
            ((u32::from(b & 0x3f) << 8) | u32::from(self.byte()?), 14)
        } else if b & 0xe0 == 0xc0 {
            let c = self.byte()?; let d = self.byte()?; let e = self.byte()?;
            ((u32::from(b & 0x1f) << 24) | (u32::from(c) << 16) |
                (u32::from(d) << 8) | u32::from(e), 29)
        } else { return Err(malformed("invalid compressed signed integer")); };
        let value = (raw >> 1) as i32 - if raw & 1 == 0 { 0 } else { 1 << (bits - 1) };
        if (bits == 14 && (-64..64).contains(&value)) ||
           (bits == 29 && (-8192..8192).contains(&value)) {
            return Err(malformed("noncanonical compressed signed integer"));
        }
        Ok(value)
    }
    fn method(&mut self, names: &mut Names<'_, '_, '_>, depth: usize) -> Result<MethodSignature> {
        if depth > MAX_SIGNATURE_DEPTH { return Err(limit("signature nesting exceeds cap")); }
        let header = self.byte()?;
        if header & 0x0f > 5 || header & 0x80 != 0 {
            return Err(malformed("invalid calling convention in method signature"));
        }
        let generic = if header & 0x10 != 0 { self.compressed()? } else { 0 };
        let count = self.compressed()? as usize;
        if count > 65_535 { return Err(limit("method arity exceeds cap")); }
        let return_type = self.ty(names, depth + 1)?;
        let mut params = names.budget.vec::<String>(count)?;
        let mut required = count;
        for _ in 0..count {
            if self.bytes.get(self.pos) == Some(&0x41) {
                if required != count { return Err(malformed("duplicate vararg sentinel")); }
                required = params.len(); self.pos += 1;
            }
            params.push(self.ty(names, depth + 1)?);
        }
        let joined = names.budget.join(&params, ", ")?;
        let formatted = names.budget.text(format_args!("({joined}):{return_type}[{header:02X}:{generic}:{required}]"))?;
        Ok(MethodSignature {
            formatted,
            return_integral: integral(&return_type),
            params_integral: params.iter().all(|p| integral(p)),
            params,
        })
    }
    fn ty(&mut self, names: &mut Names<'_, '_, '_>, depth: usize) -> Result<String> {
        if depth > MAX_SIGNATURE_DEPTH { return Err(limit("signature nesting exceeds cap")); }
        let code = self.byte()?;
        let primitive = match code {
            0x01 => Some("Void"), 0x02 => Some("Boolean"), 0x03 => Some("Char"),
            0x04 => Some("SByte"), 0x05 => Some("Byte"), 0x06 => Some("Int16"),
            0x07 => Some("UInt16"), 0x08 => Some("Int32"), 0x09 => Some("UInt32"),
            0x0a => Some("Int64"), 0x0b => Some("UInt64"), 0x0c => Some("Single"),
            0x0d => Some("Double"), 0x0e => Some("String"), 0x16 => Some("TypedReference"),
            0x18 => Some("IntPtr"), 0x19 => Some("UIntPtr"), 0x1c => Some("Object"),
            _ => None,
        };
        if let Some(s) = primitive { return names.budget.owned(s); }
        match code {
            0x0f => { let ty = self.ty(names, depth + 1)?; names.text(format_args!("{ty}*")) },
            0x10 => { let ty = self.ty(names, depth + 1)?; names.text(format_args!("{ty}&")) },
            0x11 | 0x12 => names.entity(coded_token("TypeDefOrRef", self.compressed()?)?),
            0x13 => names.text(format_args!("!{}", self.compressed()?)),
            0x1e => names.text(format_args!("!!{}", self.compressed()?)),
            0x1d => { let ty = self.ty(names, depth + 1)?; names.text(format_args!("{ty}[]")) },
            0x45 => { let ty = self.ty(names, depth + 1)?; names.text(format_args!("{ty} pinned")) },
            0x1f | 0x20 => {
                let modifier = names.entity(coded_token("TypeDefOrRef", self.compressed()?)?)?;
                let next = self.ty(names, depth + 1)?;
                let label = if code == 0x1f { "modreq" } else { "modopt" };
                names.text(format_args!("{next} {label}({modifier})"))
            }
            0x15 => {
                let lead = self.byte()?;
                if lead != 0x11 && lead != 0x12 { return Err(malformed("invalid generic instance element kind")); }
                let container = names.entity(coded_token("TypeDefOrRef", self.compressed()?)?)?;
                let count = self.compressed()? as usize;
                if count > 256 { return Err(limit("generic argument count exceeds cap")); }
                let mut args = names.budget.vec::<String>(count)?;
                for _ in 0..count { args.push(self.ty(names, depth + 1)?); }
                let joined = names.join(&args, ", ")?;
                names.text(format_args!("{container}<{joined}>"))
            }
            0x14 => {
                let element = self.ty(names, depth + 1)?;
                let rank = self.compressed()?;
                let ns = self.compressed()? as usize;
                if rank > 256 || ns > 256 { return Err(limit("array signature dimensions exceed cap")); }
                let mut sizes = names.budget.vec::<String>(ns)?;
                for _ in 0..ns { sizes.push(names.text(format_args!("{}", self.compressed()?))?); }
                let nb = self.compressed()? as usize;
                if nb > 256 { return Err(limit("array lower bounds exceed cap")); }
                let mut bounds = names.budget.vec::<String>(nb)?;
                for _ in 0..nb { bounds.push(names.text(format_args!("{}", self.signed()?))?); }
                let bounds = names.join(&bounds, ",")?;
                let sizes = names.join(&sizes, ",")?;
                names.text(format_args!("{element}[rank:{rank}; bounds:{bounds}; sizes:{sizes}]"))
            }
            0x1b => {
                let method = self.method(names, depth + 1)?;
                names.text(format_args!("fn{}", method.formatted))
            },
            _ => Err(unsupported("unknown ECMA signature element type")),
        }
    }
}
fn integral(value: &str) -> bool {
    matches!(value, "Boolean" | "Char" | "SByte" | "Byte" | "Int16" | "UInt16" |
        "Int32" | "UInt32" | "Int64" | "UInt64" | "IntPtr" | "UIntPtr")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compressed_integer_width_and_rejection() {
        let mut p = SigReader::new(&[0x7f, 0x80, 0x80, 0xc0, 0, 0x40, 0]);
        assert_eq!(p.compressed().unwrap(), 127);
        assert_eq!(p.compressed().unwrap(), 128);
        assert_eq!(p.compressed().unwrap(), 0x4000);
        assert!(SigReader::new(&[0xe0]).compressed().is_err());
        assert!(SigReader::new(&[0xc0, 0x12]).compressed().is_err());
        assert!(SigReader::new(&[0x80, 0x01]).compressed().is_err());
        assert!(SigReader::new(&[0xc0, 0, 0, 0x7f]).compressed().is_err());
        assert_eq!(SigReader::new(&[0x80, 0x01]).signed().unwrap(), -8192);
        assert_eq!(SigReader::new(&[0xc0, 0, 0, 1]).signed().unwrap(), -268435456);
        assert_eq!(SigReader::new(&[0xbf, 0x7f]).signed().unwrap(), -65);
        assert!(SigReader::new(&[0xbf, 0xff]).signed().is_err());
        assert!(SigReader::new(&[0xdf, 0xff, 0xff, 0xff]).signed().is_err());
    }
}
