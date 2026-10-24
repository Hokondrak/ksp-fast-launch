//! Assembling Mono's AOT output into sections, symbols and fixups (unresolved references, resolved by [`crate::pe`]
//! once section addresses are known).

use crate::expr::{self, Expr};
use std::collections::HashMap;

pub struct Section {
    pub name: String,
    pub data: Vec<u8>,
    /// Never written to the image (CodeView debug info).
    pub discarded: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixupKind {
    /// A little-endian value of this many bytes: the expression's value, an address or an RVA.
    Data(u8),
    /// A 32-bit displacement relative to the end of the field (the next instruction): `call`, `jmp *x(%rip)`.
    PcRel32,
}

pub struct Fixup {
    pub section: usize,
    pub offset: usize,
    pub kind: FixupKind,
    pub expr: Expr,
    pub line: usize,
}

#[derive(Default)]
pub struct Module {
    pub sections: Vec<Section>,
    /// Label -> (section, offset).
    pub symbols: HashMap<String, (usize, usize)>,
    /// `name = expression`.
    pub assignments: HashMap<String, Expr>,
    pub globals: Vec<String>,
    pub fixups: Vec<Fixup>,
}

impl Module {
    fn section(&mut self, name: &str) -> usize {
        if let Some(i) = self.sections.iter().position(|s| s.name == name) {
            return i;
        }
        self.sections.push(Section { name: name.to_string(), data: Vec::new(), discarded: name.starts_with(".debug") });
        self.sections.len() - 1
    }
}

struct Assembler {
    m: Module,
    current: usize,
    line: usize,
}

pub fn assemble(source: &str) -> Result<Module, String> {
    let mut a = Assembler { m: Module::default(), current: 0, line: 0 };
    a.current = a.m.section(".text");
    for (n, raw) in source.lines().enumerate() {
        a.line = n + 1;
        for stmt in split_statements(raw) {
            a.statement(stmt).map_err(|e| format!("line {}: {}: {}", a.line, e, raw.trim()))?;
        }
    }
    Ok(a.m)
}

/// A line may hold several statements separated by ';' (".def x; .scl 3; .type 32; .endef").
fn split_statements(line: &str) -> Vec<&str> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') || line.starts_with("//") {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_str = false;
    let mut prev = '\0';
    for (i, c) in line.char_indices() {
        if c == '"' && prev != '\\' {
            in_str = !in_str;
        } else if c == ';' && !in_str {
            out.push(line[start..i].trim());
            start = i + 1;
        }
        prev = c;
    }
    out.push(line[start..].trim());
    out.retain(|s| !s.is_empty());
    out
}

impl Assembler {
    fn here(&self) -> usize {
        self.m.sections[self.current].data.len()
    }

    fn emit(&mut self, bytes: &[u8]) {
        let cur = self.current;
        self.m.sections[cur].data.extend_from_slice(bytes);
    }

    fn fixup(&mut self, kind: FixupKind, expr: Expr) {
        let width = match kind {
            FixupKind::Data(w) => w as usize,
            FixupKind::PcRel32 => 4,
        };
        let offset = self.here();
        self.m.fixups.push(Fixup { section: self.current, offset, kind, expr, line: self.line });
        self.emit(&vec![0u8; width]);
    }

    fn statement(&mut self, s: &str) -> Result<(), String> {
        // Label, possibly followed by more on the same line.
        if let Some(colon) = label_end(s) {
            let name = &s[..colon];
            if self.m.symbols.insert(name.to_string(), (self.current, self.here())).is_some() {
                return Err(format!("label {} defined twice", name));
            }
            let rest = s[colon + 1..].trim();
            return if rest.is_empty() { Ok(()) } else { self.statement(rest) };
        }
        // name = expression
        if let Some(eq) = assignment(s) {
            let name = s[..eq].trim().to_string();
            let value = expr::parse(&s[eq + 1..])?;
            self.m.assignments.insert(name, value);
            return Ok(());
        }
        let (word, rest) = match s.find(char::is_whitespace) {
            Some(i) => (&s[..i], s[i..].trim()),
            None => (s, ""),
        };
        match word {
            ".section" => {
                let name = rest.split(',').next().unwrap_or("").trim().trim_matches('"');
                if name.is_empty() {
                    return Err(".section without a name".into());
                }
                self.current = self.m.section(name);
            }
            ".text" | ".data" | ".bss" => self.current = self.m.section(word),
            ".balign" | ".p2align" => {
                let ops = expr::split_operands(rest);
                let n = const_value(ops.first().copied().unwrap_or(""))?;
                let align = if word == ".p2align" { 1usize << n } else { n as usize };
                if align == 0 || !align.is_power_of_two() || align > 4096 {
                    return Err(format!("unsupported alignment {}", align));
                }
                let fill = match ops.get(1) {
                    Some(f) if !f.is_empty() => const_value(f)? as u8,
                    _ => 0,
                };
                while self.here() % align != 0 {
                    self.emit(&[fill]);
                }
            }
            ".byte" => self.data(rest, 1)?,
            ".word" | ".short" | ".hword" => self.data(rest, 2)?,
            ".long" | ".int" => self.data(rest, 4)?,
            ".quad" => self.data(rest, 8)?,
            ".skip" | ".space" | ".zero" => {
                let ops = expr::split_operands(rest);
                let n = const_value(ops.first().copied().unwrap_or(""))?;
                if !(0..=1 << 30).contains(&n) {
                    return Err(format!("bad size {}", n));
                }
                let fill = match ops.get(1) {
                    Some(f) => const_value(f)? as u8,
                    None => 0,
                };
                self.emit(&vec![fill; n as usize]);
            }
            ".string" | ".asciz" | ".ascii" => {
                for op in expr::split_operands(rest) {
                    let mut bytes = string_literal(op)?;
                    if word != ".ascii" {
                        bytes.push(0);
                    }
                    self.emit(&bytes);
                }
            }
            ".globl" | ".global" => {
                for name in rest.split(',') {
                    self.m.globals.push(name.trim().to_string());
                }
            }
            // Symbol-table and listing directives carry nothing the image needs.
            ".def" | ".scl" | ".type" | ".endef" | ".size" | ".file" | ".ident" | ".local" => {}
            ".secrel32" | ".secidx" => {
                // Section-relative references only occur in CodeView debug info, which is not written.
                if !self.m.sections[self.current].discarded {
                    return Err(format!("{} outside a debug section", word));
                }
                let width = if word == ".secrel32" { 4 } else { 2 };
                self.emit(&vec![0u8; width]);
            }
            "call" => {
                if rest.starts_with('*') {
                    return Err("indirect call".into());
                }
                self.emit(&[0xE8]);
                self.fixup(FixupKind::PcRel32, expr::parse(rest)?);
            }
            "jmp" => {
                // jmp *symbol+offset(%rip)
                let inner = rest
                    .strip_prefix('*')
                    .and_then(|r| r.strip_suffix("(%rip)"))
                    .ok_or_else(|| "only 'jmp *symbol(%rip)' is supported".to_string())?;
                self.emit(&[0xFF, 0x25]);
                self.fixup(FixupKind::PcRel32, expr::parse(inner)?);
            }
            _ => return Err(format!("unsupported statement '{}'", word)),
        }
        Ok(())
    }

    fn data(&mut self, rest: &str, width: u8) -> Result<(), String> {
        for op in expr::split_operands(rest) {
            let e = expr::parse(op)?;
            if e.is_constant() {
                let bytes = e.constant.to_le_bytes();
                check_fits(e.constant, width)?;
                self.emit(&bytes[..width as usize]);
            } else {
                self.fixup(FixupKind::Data(width), e);
            }
        }
        Ok(())
    }
}

/// Position of the ':' ending a leading label, if the statement starts with one.
fn label_end(s: &str) -> Option<usize> {
    let mut chars = s.char_indices();
    let (_, first) = chars.next()?;
    if !(first.is_ascii_alphabetic() || first == '_' || first == '.' || first == '$') {
        return None;
    }
    for (i, c) in chars {
        if c == ':' {
            return Some(i);
        }
        if !(c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$') {
            return None;
        }
    }
    None
}

/// Position of '=' in `name = expression` (and not in a directive).
fn assignment(s: &str) -> Option<usize> {
    let eq = s.find('=')?;
    let name = s[..eq].trim();
    let ok = !name.is_empty() && !name.contains(char::is_whitespace) && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.' || c == '$');
    ok.then_some(eq)
}

fn const_value(s: &str) -> Result<i64, String> {
    let e = expr::parse(s)?;
    if !e.is_constant() {
        return Err(format!("'{}' must be a constant", s));
    }
    Ok(e.constant)
}

/// Accepts the value if it fits `width` bytes as either a signed or an unsigned number.
pub fn check_fits(v: i64, width: u8) -> Result<(), String> {
    if width >= 8 {
        return Ok(());
    }
    let bits = 8 * width as u32;
    let min = -(1i64 << (bits - 1));
    let max = (1i64 << bits) - 1;
    if v < min || v > max {
        return Err(format!("value {} does not fit in {} bytes", v, width));
    }
    Ok(())
}

fn string_literal(op: &str) -> Result<Vec<u8>, String> {
    let inner = op.strip_prefix('"').and_then(|s| s.strip_suffix('"')).ok_or_else(|| format!("expected a string, got {}", op))?;
    let mut out = Vec::new();
    let b = inner.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'\\' {
            out.push(b[i]);
            i += 1;
            continue;
        }
        i += 1;
        let c = *b.get(i).ok_or("string ends with a backslash")?;
        i += 1;
        match c {
            b'n' => out.push(b'\n'),
            b't' => out.push(b'\t'),
            b'r' => out.push(b'\r'),
            b'b' => out.push(8),
            b'f' => out.push(12),
            b'"' | b'\\' => out.push(c),
            b'0'..=b'7' => {
                let mut v = (c - b'0') as u32;
                for _ in 0..2 {
                    match b.get(i) {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + (d - b'0') as u32;
                            i += 1;
                        }
                        _ => break,
                    }
                }
                out.push(v as u8);
            }
            b'x' => {
                let start = i;
                while i < b.len() && (b[i] as char).is_ascii_hexdigit() {
                    i += 1;
                }
                let v = u32::from_str_radix(std::str::from_utf8(&b[start..i]).unwrap_or(""), 16).map_err(|_| "bad \\x escape")?;
                out.push(v as u8);
            }
            other => return Err(format!("unsupported escape \\{}", other as char)),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_data_labels_and_instructions() {
        let src = "\
.section .text
\t.balign 16
f:
\t.byte 0x48,0x89,0xe5
call g
g:
jmp *got+8(%rip)
\t.long 345
.section .rdata
s:
\t.string \"a\\\"b\\101\"
\t.word 2, 4431
x=g - f
.section .debug$S
\t.secrel32 f
\t.secidx f
";
        let m = assemble(src).unwrap();
        let text = &m.sections[0];
        assert_eq!(&text.data[..4], &[0x48, 0x89, 0xe5, 0xE8]);
        assert_eq!(&text.data[8..10], &[0xFF, 0x25]);
        assert_eq!(&text.data[14..18], &345u32.to_le_bytes());
        assert_eq!(m.symbols["g"], (0, 8));
        let rdata = &m.sections[1];
        assert_eq!(&rdata.data[..5], b"a\"bA\0");
        assert_eq!(&rdata.data[5..9], &[2, 0, 0x4F, 0x11]);
        assert_eq!(m.fixups.len(), 2);
        assert_eq!(m.fixups[0].kind, FixupKind::PcRel32);
        assert!(m.assignments.contains_key("x"));
        assert!(m.sections[2].discarded);
    }

    #[test]
    fn rejects_anything_unexpected() {
        for bad in ["mov %rax, %rbx", "call *%rax", "jmp label", ".comm x,8", "\t.secrel32 f", "\t.byte 300", "a: \na:"] {
            assert!(assemble(bad).is_err(), "accepted: {}", bad);
        }
    }

    #[test]
    fn splits_statements() {
        assert_eq!(split_statements("\t.def a; .scl 3; .type 32; .endef"), vec![".def a", ".scl 3", ".type 32", ".endef"]);
        assert_eq!(split_statements("\t.string \"a;b\""), vec![".string \"a;b\""]);
        assert!(split_statements("   ").is_empty());
    }
}
