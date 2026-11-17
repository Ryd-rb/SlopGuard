//! Monotonic per-request allocation accounting. Charges precede every owned
//! untrusted string, signature expansion, IL record, and map/cache entry.
//! This is a structural upper bound, not an RSS measurement; allocator overhead
//! and the caller-provided input byte slices are deliberately separate.
use crate::{CheckError, ErrorKind, Result};
use std::{cell::Cell, fmt, mem};

pub(crate) const MAX_WORK_BYTES: usize = 32 * 1024 * 1024;
pub(crate) const MAX_TEXT_BYTES: usize = 2 * 1024 * 1024;
pub(crate) const MAX_JSON_BYTES: usize = 8 * 1024 * 1024;

fn exhausted() -> CheckError {
    CheckError::new(ErrorKind::Limit, "structural allocation budget exhausted")
}

pub(crate) struct Budget {
    used: Cell<usize>,
    limit: usize,
}
impl Budget {
    pub(crate) fn new() -> Self { Self { used: Cell::new(0), limit: MAX_WORK_BYTES } }
    pub(crate) fn used(&self) -> usize { self.used.get() }
    pub(crate) fn charge(&self, size: usize) -> Result<()> {
        let next = self.used().checked_add(size).ok_or_else(exhausted)?;
        if next > self.limit { return Err(exhausted()); }
        self.used.set(next);
        Ok(())
    }
    pub(crate) fn charge_array<T>(&self, count: usize) -> Result<()> {
        self.charge(count.checked_mul(mem::size_of::<T>()).ok_or_else(exhausted)?)
    }
    pub(crate) fn vec<T>(&self, capacity: usize) -> Result<Vec<T>> {
        self.charge_array::<T>(capacity)?;
        let mut result = Vec::new();
        result.try_reserve_exact(capacity).map_err(|_| exhausted())?;
        Ok(result)
    }
    // Hash table bucket/control allocations are conservatively charged at 2x
    // the pair storage plus 32 bytes per element before requesting capacity.
    pub(crate) fn map_entries<K, V>(&self, count: usize) -> Result<()> {
        let unit = mem::size_of::<(K, V)>().checked_add(32).ok_or_else(exhausted)?;
        self.charge(count.checked_mul(unit).and_then(|x| x.checked_mul(2)).ok_or_else(exhausted)?)
    }
    pub(crate) fn owned(&self, value: &str) -> Result<String> {
        if value.len() > MAX_TEXT_BYTES { return Err(exhausted()); }
        self.charge(value.len())?;
        let mut out = String::new();
        out.try_reserve_exact(value.len()).map_err(|_| exhausted())?;
        out.push_str(value);
        Ok(out)
    }
    // Format twice: first count bytes without allocating, then reserve exactly
    // once and render. Even a DAG of exponentially expanding TypeSpecs fails
    // the counting pass before format! would allocate the expansion.
    pub(crate) fn text(&self, arguments: fmt::Arguments<'_>) -> Result<String> {
        struct Counter { len: usize }
        impl fmt::Write for Counter {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.len = self.len.checked_add(s.len()).ok_or(fmt::Error)?;
                if self.len > MAX_TEXT_BYTES { return Err(fmt::Error); }
                Ok(())
            }
        }
        let mut counter = Counter { len: 0 };
        fmt::write(&mut counter, arguments.clone()).map_err(|_| exhausted())?;
        self.charge(counter.len)?;
        let mut out = String::new();
        out.try_reserve_exact(counter.len).map_err(|_| exhausted())?;
        fmt::write(&mut out, arguments).map_err(|_| exhausted())?;
        Ok(out)
    }
    pub(crate) fn join(&self, values: &[String], delimiter: &str) -> Result<String> {
        let separators = values.len().saturating_sub(1).checked_mul(delimiter.len()).ok_or_else(exhausted)?;
        let length = values.iter().try_fold(separators, |n, s|
            n.checked_add(s.len()).ok_or_else(exhausted))?;
        if length > MAX_TEXT_BYTES { return Err(exhausted()); }
        self.charge(length)?;
        let mut out = String::new();
        out.try_reserve_exact(length).map_err(|_| exhausted())?;
        for (i, s) in values.iter().enumerate() {
            if i != 0 { out.push_str(delimiter); }
            out.push_str(s);
        }
        Ok(out)
    }
    pub(crate) fn quoted(&self, value: &str) -> Result<String> {
        let len = quoted_len(value)?;
        if len > MAX_TEXT_BYTES { return Err(exhausted()); }
        self.charge(len)?;
        let mut out = String::new();
        out.try_reserve_exact(len).map_err(|_| exhausted())?;
        append_quoted(&mut out, value);
        Ok(out)
    }
    #[cfg(test)]
    fn with_limit(limit: usize) -> Self { Self { used: Cell::new(0), limit } }
}

// System.Text.Json's default JavaScript encoder (.NET 10): escapes HTML-sensitive
// ASCII, non-ASCII UTF-16 units, and controls; quote / backtick are
// six-character hex escapes, while backslash and common controls use
// two-character short forms. No UnsafeRelaxedJsonEscaping.
pub(crate) fn quoted_len(value: &str) -> Result<usize> {
    let mut len = 2usize;
    for ch in value.chars() {
        let unit = match ch {
            '\\' | '\n' | '\r' | '\t' | '\u{0008}' | '\u{000c}' => 2,
            ch if ch.is_control() || matches!(ch, '"' | '`' | '<' | '>' | '&' | '\'' | '+') || !ch.is_ascii() => {
                if ch.len_utf16() == 2 { 12 } else { 6 }
            }
            _ => 1,
        };
        len = len.checked_add(unit).ok_or_else(exhausted)?;
        if len > MAX_JSON_BYTES { return Err(exhausted()); }
    }
    Ok(len)
}
fn append_u16_hex(out: &mut String, unit: u16) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    out.push_str("\\u");
    for shift in [12u16, 8, 4, 0] {
        out.push(char::from(HEX[((unit >> shift) & 15) as usize]));
    }
}
pub(crate) fn append_quoted(out: &mut String, value: &str) {
    out.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\u{0008}' => out.push_str("\\b"),
            '\u{000c}' => out.push_str("\\f"),
            ch if ch.is_control() || matches!(ch, '"' | '`' | '<' | '>' | '&' | '\'' | '+') || !ch.is_ascii() => {
                let mut units = [0u16; 2];
                for unit in ch.encode_utf16(&mut units) { append_u16_hex(out, *unit); }
            }
            _ => out.push(ch),
        }
    }
    out.push('"');
}

// Final result is independently capped, including JSON escapes, BEFORE each
// append; no post-hoc check on an already oversize String.
pub(crate) struct JsonOut { text: String }
impl JsonOut {
    pub(crate) fn new() -> Self { Self { text: String::new() } }
    fn reserve(&mut self, n: usize) -> Result<()> {
        if n > MAX_JSON_BYTES.saturating_sub(self.text.len()) { return Err(exhausted()); }
        self.text.try_reserve(n).map_err(|_| exhausted())?;
        Ok(())
    }
    pub(crate) fn push(&mut self, s: &str) -> Result<()> {
        self.reserve(s.len())?;
        self.text.push_str(s); Ok(())
    }
    pub(crate) fn number(&mut self, value: impl fmt::Display) -> Result<()> {
        struct Counter { len: usize }
        impl fmt::Write for Counter {
            fn write_str(&mut self, s: &str) -> fmt::Result {
                self.len = self.len.checked_add(s.len()).ok_or(fmt::Error)?; Ok(())
            }
        }
        let mut counter = Counter { len: 0 };
        fmt::write(&mut counter, format_args!("{value}")).map_err(|_| exhausted())?;
        self.reserve(counter.len)?;
        use std::fmt::Write;
        write!(self.text, "{value}").map_err(|_| exhausted())
    }
    pub(crate) fn quoted(&mut self, s: &str) -> Result<()> {
        let len = quoted_len(s)?;
        self.reserve(len)?;
        append_quoted(&mut self.text, s);
        Ok(())
    }
    pub(crate) fn finish(self) -> String { self.text }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn default_system_text_json_quoting_is_exact() {
        let budget = Budget::new();
        let s = "\"\\<>&'+`\n\r\t\u{0008}\u{000c}\u{0001}é😀\u{2028}";
        assert_eq!(budget.quoted(s).unwrap(),
            r#""\u0022\\\u003C\u003E\u0026\u0027\u002B\u0060\n\r\t\b\f\u0001\u00E9\uD83D\uDE00\u2028""#);
    }
    #[test]
    fn charges_before_format_expansion_and_repeated_large_strings() {
        let budget = Budget::with_limit(64);
        let once = budget.owned(&"x".repeat(32)).unwrap();
        assert!(budget.owned(&once).is_ok());
        assert_eq!(budget.used(), 64);
        assert_eq!(budget.owned(&once).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(budget.used(), 64);
        let tiny = Budget::with_limit(16);
        let huge = "m".repeat(256);
        assert_eq!(tiny.text(format_args!("{huge}{huge}")).unwrap_err().kind, ErrorKind::Limit);
        assert_eq!(tiny.used(), 0);
    }
    #[test]
    fn output_cap_is_checked_before_appending() {
        let mut out = JsonOut::new();
        out.push("[").unwrap();
        assert!(out.push(&"x".repeat(MAX_JSON_BYTES)).is_err());
        assert_eq!(out.finish(), "[");
    }
}
