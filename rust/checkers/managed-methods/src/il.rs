//! Decode IL operands without CLR loading, normalize instruction-relative
//! branch targets, locals and protected regions to match C# comparison.
use crate::{CheckError, ErrorKind, Result};
use crate::budget::Budget;
use crate::metadata::{Metadata, Names, read_u16, read_u32};
use std::collections::{HashMap, HashSet};

const MAX_IL_BYTES: usize = 1024 * 1024;
const MAX_IL_INSTRUCTIONS: usize = 200_000;
const MAX_EH_CLAUSES: usize = 4_096;
const MAX_METHOD_SECTIONS: usize = 64;

fn malformed(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Malformed, message) }
fn unsupported(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Unsupported, message) }
fn limit(message: &'static str) -> CheckError { CheckError::new(ErrorKind::Limit, message) }

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum Operand {
    InlineNone, ShortInlineBrTarget, InlineBrTarget, InlineSwitch,
    ShortInlineI, InlineI, InlineI8, ShortInlineR, InlineR,
    ShortInlineVar, InlineVar, InlineString, InlineField, InlineMethod,
    InlineSig, InlineTok, InlineType, InlinePhi,
}
include!("opcodes.rs");

#[derive(Debug)]
pub(crate) struct Body {
    pub instructions: Vec<String>,
    pub literals: Vec<i64>,
}
struct Instruction {
    offset: usize,
    name: String,
    operand: Option<String>,
    targets: Option<Vec<i64>>,
}
struct Decoded {
    instructions: Vec<Instruction>,
    positions: HashMap<i64, usize>,
    literals: Vec<i64>,
}
struct Cursor<'a> { bytes: &'a [u8], pos: usize }
impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self { Self { bytes, pos: 0 } }
    fn chunk(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self.pos.checked_add(len).ok_or_else(|| malformed("IL operand offset overflow"))?;
        let bytes = self.bytes.get(self.pos..end).ok_or_else(|| malformed("truncated IL operand"))?;
        self.pos = end; Ok(bytes)
    }
    fn byte(&mut self) -> Result<u8> { Ok(self.chunk(1)?[0]) }
    fn i8(&mut self) -> Result<i8> { Ok(self.byte()? as i8) }
    fn u16(&mut self) -> Result<u16> { Ok(u16::from_le_bytes(self.chunk(2)?.try_into().expect("checked"))) }
    fn i32(&mut self) -> Result<i32> { Ok(i32::from_le_bytes(self.chunk(4)?.try_into().expect("checked"))) }
    fn u32(&mut self) -> Result<u32> { Ok(u32::from_le_bytes(self.chunk(4)?.try_into().expect("checked"))) }
    fn i64(&mut self) -> Result<i64> { Ok(i64::from_le_bytes(self.chunk(8)?.try_into().expect("checked"))) }
}
fn decode(il: &[u8], mut names: Option<&mut Names<'_, '_, '_>>, budget: &Budget) -> Result<Decoded> {
    if il.len() > MAX_IL_BYTES { return Err(limit("IL body exceeds size cap")); }
    let mut cursor = Cursor::new(il);
    let mut instructions = Vec::new();
    let mut unique_literals = HashSet::new();
    let mut literals = Vec::new();
    while cursor.pos < il.len() {
        if instructions.len() >= MAX_IL_INSTRUCTIONS { return Err(limit("IL instruction count exceeds cap")); }
        let offset = cursor.pos;
        let b = cursor.byte()?;
        let code = if b == 0xfe { 0xfe00 | u16::from(cursor.byte()?) } else { u16::from(b) };
        let (name, kind) = opcode(code).ok_or_else(|| malformed("unknown IL opcode"))?;
        let mut operand = None;
        let mut targets = None;
        let mut literal = None;
        let name = if matches!(kind, Operand::ShortInlineBrTarget | Operand::InlineBrTarget) {
            let disp = if matches!(kind, Operand::ShortInlineBrTarget) { i64::from(cursor.i8()?) }
                else { i64::from(cursor.i32()?) };
            let mut positions = budget.vec::<i64>(1)?;
            positions.push(cursor.pos as i64 + disp);
            targets = Some(positions);
            budget.owned(name.strip_suffix(".s").unwrap_or(name))?
        } else if matches!(kind, Operand::InlineSwitch) {
            let count = cursor.i32()?;
            if count < 0 || count as usize > (il.len() - cursor.pos) / 4 {
                return Err(malformed("truncated IL switch table"));
            }
            let count = count as usize;
            let base = cursor.pos.checked_add(count * 4).ok_or_else(|| malformed("switch offset overflow"))? as i64;
            let mut entries = budget.vec::<i64>(count)?;
            for _ in 0..count { entries.push(base + i64::from(cursor.i32()?)); }
            targets = Some(entries);
            budget.owned(name)?
        } else {
            operand = match kind {
                Operand::InlineNone => None,
                Operand::ShortInlineI => Some(budget.text(format_args!("{}", cursor.i8()?))?),
                Operand::InlineI => Some(budget.text(format_args!("{}", cursor.i32()?))?),
                Operand::InlineI8 => Some(budget.text(format_args!("{}", cursor.i64()?))?),
                Operand::ShortInlineR => Some(budget.text(format_args!("{:08X}", cursor.u32()?))?),
                Operand::InlineR => Some(budget.text(format_args!("{:016X}", cursor.i64()? as u64))?),
                Operand::ShortInlineVar => Some(budget.text(format_args!("{}", cursor.byte()?))?),
                Operand::InlineVar => Some(budget.text(format_args!("{}", cursor.u16()?))?),
                Operand::InlineString => {
                    let token = cursor.u32()? & 0x00ff_ffff;
                    let value = names.as_deref_mut().ok_or_else(|| unsupported("no metadata for IL string token"))?
                        .user_string(token)?;
                    Some(budget.quoted(&value)?)
                }
                Operand::InlineField | Operand::InlineMethod | Operand::InlineSig |
                Operand::InlineTok | Operand::InlineType => {
                    let token = cursor.u32()?;
                    Some(names.as_deref_mut().ok_or_else(|| unsupported("no metadata for IL token"))?.entity(token)?)
                }
                Operand::InlinePhi => return Err(unsupported("deprecated InlinePhi operand is unsupported")),
                Operand::ShortInlineBrTarget | Operand::InlineBrTarget | Operand::InlineSwitch => unreachable!(),
            };
            budget.owned(name)?
        };
        if name == "ldc.i4.m1" { literal = Some(-1); }
        else if let Some(last) = name.strip_prefix("ldc.i4.") {
            if last.len() == 1 && last.as_bytes()[0].is_ascii_digit() && last.as_bytes()[0] <= b'8' {
                literal = Some(i64::from(last.as_bytes()[0] - b'0'));
            }
        }
        if matches!(name.as_str(), "ldc.i4" | "ldc.i4.s" | "ldc.i8") {
            literal = operand.as_deref().and_then(|raw| raw.parse::<i64>().ok());
        }
        if let Some(value) = literal {
            if !unique_literals.contains(&value) {
                budget.map_entries::<i64, ()>(1)?;
                unique_literals.try_reserve(1).map_err(|_| limit("integer literal set allocation failed"))?;
                budget.charge_array::<i64>(1)?;
                literals.try_reserve(1).map_err(|_| limit("integer literal list allocation failed"))?;
                unique_literals.insert(value);
                literals.push(value);
            }
        }
        budget.charge_array::<Instruction>(2)?;
        instructions.try_reserve(1).map_err(|_| limit("IL instruction allocation failed"))?;
        instructions.push(Instruction { offset, name, operand, targets });
    }
    budget.map_entries::<i64, usize>(instructions.len().saturating_add(1))?;
    let mut positions = HashMap::new();
    positions.try_reserve(instructions.len().saturating_add(1))
        .map_err(|_| limit("IL branch position map allocation failed"))?;
    for (i, instruction) in instructions.iter().enumerate() {
        if positions.insert(instruction.offset as i64, i).is_some() {
            return Err(malformed("duplicate IL instruction offset"));
        }
    }
    positions.insert(il.len() as i64, instructions.len());
    Ok(Decoded { instructions, positions, literals })
}
fn position(positions: &HashMap<i64, usize>, offset: i64) -> Result<usize> {
    positions.get(&offset).copied().ok_or_else(|| malformed("branch or EH offset is not on an instruction boundary"))
}
#[derive(Clone, Copy, Debug)]
struct Region {
    kind: u32, try_at: u32, try_len: u32, handler_at: u32, handler_len: u32, token: u32,
}
fn parse_regions(raw: &[u8], start: usize, budget: &Budget) -> Result<Vec<Region>> {
    let mut regions = Vec::new();
    let mut cursor = start;
    let mut more = true;
    let mut sections = 0usize;
    while more {
        sections += 1;
        if sections > MAX_METHOD_SECTIONS { return Err(limit("method data section count exceeds cap")); }
        let header = raw.get(cursor..cursor.checked_add(4).ok_or_else(|| malformed("extra section overflow"))?)
            .ok_or_else(|| malformed("truncated method section header"))?;
        let kind = header[0];
        more = kind & 0x80 != 0;
        let fat = kind & 0x40 != 0;
        if kind & 0x3f != 1 { return Err(unsupported("unsupported method data section kind")); }
        let size = (if fat {
            u32::from(header[1]) | (u32::from(header[2]) << 8) | (u32::from(header[3]) << 16)
        } else { u32::from(header[1]) }) as usize;
        let stride = if fat { 24 } else { 12 };
        if size < 4 || (size - 4) % stride != 0 { return Err(malformed("invalid exception section length")); }
        let data = raw.get(cursor + 4..cursor.checked_add(size).ok_or_else(|| malformed("exception section overflow"))?)
            .ok_or_else(|| malformed("truncated exception section"))?;
        let clauses = (size - 4) / stride;
        if regions.len().saturating_add(clauses) > MAX_EH_CLAUSES { return Err(limit("exception region count exceeds cap")); }
        budget.charge_array::<Region>(clauses.saturating_mul(2))?;
        regions.try_reserve(clauses).map_err(|_| limit("exception region allocation failed"))?;
        for clause in data.chunks_exact(stride) {
            let region = if fat {
                Region { kind: read_u32(clause, 0)?, try_at: read_u32(clause, 4)?, try_len: read_u32(clause, 8)?,
                    handler_at: read_u32(clause, 12)?, handler_len: read_u32(clause, 16)?, token: read_u32(clause, 20)? }
            } else {
                Region { kind: u32::from(read_u16(clause, 0)?), try_at: u32::from(read_u16(clause, 2)?),
                    try_len: u32::from(clause[4]), handler_at: u32::from(read_u16(clause, 5)?),
                    handler_len: u32::from(clause[7]), token: read_u32(clause, 8)? }
            };
            regions.push(region);
        }
        cursor = cursor.checked_add(size).ok_or_else(|| malformed("exception section offset overflow"))?;
        if more { cursor = cursor.checked_add(3).ok_or_else(|| malformed("method section alignment overflow"))? & !3; }
    }
    Ok(regions)
}

pub(crate) fn body(pe: &Metadata<'_>, names: &mut Names<'_, '_, '_>, rva: u32, budget: &Budget) -> Result<Body> {
    let raw = pe.rva_tail(rva)?;
    let lead = *raw.first().ok_or_else(|| malformed("method body is missing"))?;
    let (header_size, size, init_locals, local_sig, more_sections) = if lead & 3 == 2 {
        (1usize, usize::from(lead >> 2), false, 0u32, false)
    } else if lead & 3 == 3 {
        let flags = read_u16(raw, 0)?;
        let header_size = usize::from(flags >> 12) * 4;
        if header_size < 12 { return Err(malformed("fat method header is too short")); }
        raw.get(..header_size).ok_or_else(|| malformed("truncated fat method header"))?;
        let size = read_u32(raw, 4)? as usize;
        (header_size, size, flags & 0x10 != 0, read_u32(raw, 8)?, flags & 8 != 0)
    } else { return Err(malformed("invalid method header encoding")); };
    if size > MAX_IL_BYTES { return Err(limit("method IL byte count exceeds cap")); }
    let il_end = header_size.checked_add(size).ok_or_else(|| malformed("IL length overflow"))?;
    let il = raw.get(header_size..il_end).ok_or_else(|| malformed("truncated IL method body"))?;
    let decoded = decode(il, Some(&mut *names), budget)?;
    let mut normalized = Vec::new();
    // Vec storage and each string must be charged before the append. In
    // particular there is no uncharged intermediate collected targets array.
    if local_sig != 0 {
        let local_types = names.local_token(local_sig)?;
        let joined = names.join(&local_types, ", ")?;
        let label = if init_locals { "locals init " } else { "locals " };
        let text = names.text(format_args!("{label}{joined}"))?;
        push_normalized(&mut normalized, text, budget)?;
    }
    for ins in &decoded.instructions {
        let operand = if let Some(targets) = &ins.targets {
            let mut parts = budget.vec::<String>(targets.len())?;
            for &at in targets {
                let index = position(&decoded.positions, at)?;
                parts.push(names.text(format_args!("#{index}"))?);
            }
            Some(names.join(&parts, ", ")?)
        } else { None };
        let text = match operand.as_deref().or(ins.operand.as_deref()) {
            Some(operand) => names.text(format_args!("{} {operand}", ins.name))?,
            None => names.text(format_args!("{}", ins.name))?,
        };
        push_normalized(&mut normalized, text, budget)?;
    }
    if more_sections {
        let sections_start = il_end.checked_add(3).ok_or_else(|| malformed("section alignment overflow"))? & !3;
        for region in parse_regions(raw, sections_start, budget)? {
            let try_end = region.try_at.checked_add(region.try_len).ok_or_else(|| malformed("try extent overflow"))?;
            let handler_end = region.handler_at.checked_add(region.handler_len).ok_or_else(|| malformed("handler extent overflow"))?;
            let annotation = match region.kind {
                0 => names.entity(region.token)?,
                1 => names.text(format_args!("#{}", position(&decoded.positions, i64::from(region.token))?))?,
                2 | 4 => String::new(),
                _ => return Err(malformed("unknown exception region flags")),
            };
            let label = match region.kind { 0 => "Catch", 1 => "Filter", 2 => "Finally", 4 => "Fault", _ => unreachable!() };
            let try_start = position(&decoded.positions, i64::from(region.try_at))?;
            let try_end = position(&decoded.positions, i64::from(try_end))?;
            let handler_start = position(&decoded.positions, i64::from(region.handler_at))?;
            let handler_end = position(&decoded.positions, i64::from(handler_end))?;
            let text = names.text(format_args!("{label} #{try_start}..#{try_end} #{handler_start}..#{handler_end} {annotation}"))?;
            push_normalized(&mut normalized, text, budget)?;
        }
    }
    Ok(Body { instructions: normalized, literals: decoded.literals })
}
fn push_normalized(body: &mut Vec<String>, text: String, budget: &Budget) -> Result<()> {
    budget.charge_array::<String>(2)?;
    body.try_reserve(1).map_err(|_| limit("normalized IL vector allocation failed"))?;
    body.push(text);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_operand_widths() {
        for input in [&[0x1f][..], &[0x20, 1, 2][..], &[0x21, 1, 2, 3][..],
            &[0x22, 1][..], &[0x23, 1, 2, 3][..], &[0xfe][..], &[0x38, 1, 2][..],
            &[0x45, 1, 0, 0, 0, 0][..]] {
            assert_eq!(decode(input, None, &Budget::new()).err().expect("must fail").kind, ErrorKind::Malformed);
        }
    }
    #[test]
    fn keeps_signed_literals_and_short_branches() {
        let result = decode(&[0x1f, 0xfd, 0x2b, 0x02, 0x17, 0x2a, 0x18, 0x2a], None, &Budget::new()).unwrap();
        assert_eq!(result.literals, vec![-3, 1, 2]);
        let first_branch = result.instructions[1].targets.as_ref().unwrap()[0];
        assert_eq!(position(&result.positions, first_branch).unwrap(), 4);
    }
    #[test]
    fn malformed_branch_alignment_is_not_silently_rounded() {
        let result = decode(&[0x2b, 0xff, 0x2a], None, &Budget::new()).unwrap();
        assert!(position(&result.positions, result.instructions[0].targets.as_ref().unwrap()[0]).is_err());
    }
    #[test]
    fn method_data_sections_are_bounded_and_truncated_sections_fail() {
        let mut repeated = Vec::new();
        for _ in 0..MAX_METHOD_SECTIONS + 1 { repeated.extend_from_slice(&[0x81, 4, 0, 0]); }
        assert_eq!(parse_regions(&repeated, 0, &Budget::new()).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(parse_regions(&[0x01, 16, 0, 0], 0, &Budget::new()).unwrap_err().kind, ErrorKind::Malformed);
    }
    #[test]
    fn invalid_opcode_is_malformed() { assert_eq!(decode(&[0xfe, 0xff], None, &Budget::new()).err().unwrap().kind, ErrorKind::Malformed); }
}
