//! Lays out an assembled module and writes it as a PE32+ DLL: no imports and no entry point, base relocations for
//! every 64-bit absolute address (so the image loads at any address), `.pdata` as the exception directory (Windows
//! unwinds AOT frames from it), and an export table for the module's global symbols.

use crate::asm::{check_fits, FixupKind, Module};
use crate::expr::{Expr, Term};
use std::collections::HashMap;

const IMAGE_BASE: u64 = 0x1_8000_0000;
const SECTION_ALIGN: u32 = 0x1000;
const FILE_ALIGN: u32 = 0x200;

const CODE: u32 = 0x6000_0020; // CNT_CODE | MEM_EXECUTE | MEM_READ
const RDATA: u32 = 0x4000_0040; // CNT_INITIALIZED_DATA | MEM_READ
const DATA: u32 = 0xC000_0040; // CNT_INITIALIZED_DATA | MEM_READ | MEM_WRITE
const BSS: u32 = 0xC000_0080; // CNT_UNINITIALIZED_DATA | MEM_READ | MEM_WRITE
const RELOC: u32 = 0x4200_0040; // CNT_INITIALIZED_DATA | MEM_DISCARDABLE | MEM_READ

struct Out {
    name: [u8; 8],
    data: Vec<u8>,
    virtual_size: u32,
    characteristics: u32,
    rva: u32,
    uninitialized: bool,
}

fn align(v: u32, a: u32) -> u32 {
    v.div_ceil(a) * a
}

fn characteristics(name: &str) -> Result<u32, String> {
    Ok(match name {
        ".text" => CODE,
        ".rdata" | ".xdata" | ".pdata" => RDATA,
        ".data" => DATA,
        ".bss" => BSS,
        other => return Err(format!("unexpected section {}", other)),
    })
}

fn short_name(name: &str) -> [u8; 8] {
    let mut n = [0u8; 8];
    for (i, b) in name.bytes().take(8).enumerate() {
        n[i] = b;
    }
    n
}

/// An expression's value as `abs * (image base) + rva + constant`: `abs` is 1 for an address, 0 for a difference.
struct Value {
    abs: i64,
    rva: i64,
}

struct Resolver<'a> {
    m: &'a Module,
    section_rva: &'a HashMap<usize, u32>,
}

impl Resolver<'_> {
    fn symbol_rva(&self, name: &str) -> Result<Option<i64>, String> {
        match self.m.symbols.get(name) {
            Some(&(sec, off)) => match self.section_rva.get(&sec) {
                Some(&rva) => Ok(Some(rva as i64 + off as i64)),
                None => Err(format!("{} is in a section that is not written", name)),
            },
            None => Ok(None),
        }
    }

    fn value(&self, e: &Expr, here: i64, depth: u32) -> Result<Value, String> {
        if depth > 32 {
            return Err("assignment cycle".into());
        }
        let mut v = Value { abs: 0, rva: e.constant };
        for (coef, term) in &e.terms {
            match term {
                Term::Here => {
                    v.abs += coef;
                    v.rva += coef * here;
                }
                Term::Symbol(name) => {
                    if let Some(rva) = self.symbol_rva(name)? {
                        v.abs += coef;
                        v.rva += coef * rva;
                    } else if let Some(a) = self.m.assignments.get(name) {
                        let inner = self.value(a, here, depth + 1)?;
                        v.abs += coef * inner.abs;
                        v.rva += coef * inner.rva;
                    } else {
                        return Err(format!("undefined symbol {}", name));
                    }
                }
                Term::ImageRelative(name) => {
                    let rva = self.symbol_rva(name)?.ok_or_else(|| format!("undefined symbol {}", name))?;
                    v.rva += coef * rva;
                }
            }
        }
        Ok(v)
    }
}

pub fn write(m: &Module, dll_name: &str) -> Result<Vec<u8>, String> {
    // Sections in the order the source declares them; debug info is dropped.
    let kept: Vec<usize> = (0..m.sections.len()).filter(|&i| !m.sections[i].discarded).collect();
    let has_export = !m.globals.is_empty();
    let extra = 1 + has_export as u32; // .reloc and .edata
    let headers = 0x40 + 4 + 20 + 240 + 40 * (kept.len() as u32 + extra);
    let size_of_headers = align(headers, FILE_ALIGN);

    let mut rva = SECTION_ALIGN;
    let mut section_rva = HashMap::new();
    let mut outs = Vec::new();
    for &i in &kept {
        let s = &m.sections[i];
        let ch = characteristics(&s.name)?;
        // An empty section still needs an address for labels placed in it.
        let size = s.data.len().max(1) as u32;
        section_rva.insert(i, rva);
        let uninitialized = ch == BSS && s.data.iter().all(|&b| b == 0) && !m.fixups.iter().any(|f| f.section == i);
        outs.push(Out { name: short_name(&s.name), data: s.data.clone(), virtual_size: size, characteristics: ch, rva, uninitialized });
        rva = align(rva + size, SECTION_ALIGN);
    }

    // Resolve every fixup in a written section.
    let resolver = Resolver { m, section_rva: &section_rva };
    let mut base_relocs: Vec<u32> = Vec::new();
    for f in &m.fixups {
        let Some(&sec_rva) = section_rva.get(&f.section) else { continue };
        let at = sec_rva as i64 + f.offset as i64;
        let v = resolver.value(&f.expr, at, 0).map_err(|e| format!("line {}: {}", f.line, e))?;
        let out = &mut outs[kept.iter().position(|&k| k == f.section).unwrap()];
        let field = &mut out.data[f.offset..];
        match f.kind {
            FixupKind::PcRel32 => {
                if v.abs != 1 {
                    return Err(format!("line {}: a call or jump target must be an address", f.line));
                }
                let disp = v.rva - (at + 4);
                i32::try_from(disp).map_err(|_| format!("line {}: displacement out of range", f.line))?;
                field[..4].copy_from_slice(&(disp as i32).to_le_bytes());
            }
            FixupKind::Data(width) => {
                let value = match v.abs {
                    0 => v.rva,
                    1 if width == 8 => {
                        base_relocs.push(at as u32);
                        (IMAGE_BASE as i64).wrapping_add(v.rva)
                    }
                    1 => return Err(format!("line {}: an absolute address in a {}-byte field cannot be relocated", f.line, width)),
                    n => return Err(format!("line {}: expression adds {} addresses", f.line, n)),
                };
                check_fits(value, width).map_err(|e| format!("line {}: {}", f.line, e))?;
                field[..width as usize].copy_from_slice(&value.to_le_bytes()[..width as usize]);
            }
        }
    }

    // .pdata must be sorted for the exception directory.
    let pdata = kept.iter().position(|&k| m.sections[k].name == ".pdata");
    if let Some(p) = pdata {
        let d = &outs[p].data;
        let starts: Vec<u32> = d.chunks(12).filter(|c| c.len() == 12).map(|c| u32::from_le_bytes(c[..4].try_into().unwrap())).collect();
        if starts.windows(2).any(|w| w[0] > w[1]) {
            return Err(".pdata entries are not in address order".into());
        }
    }

    // Export table.
    let mut export_dir = (0u32, 0u32);
    if has_export {
        let mut names: Vec<(&str, u32)> = Vec::new();
        for g in &m.globals {
            let r = resolver.symbol_rva(g)?.ok_or_else(|| format!("global {} is not defined", g))?;
            names.push((g.as_str(), r as u32));
        }
        names.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
        names.dedup_by(|a, b| a.0 == b.0);
        let n = names.len() as u32;
        let base = rva;
        let functions = base + 40;
        let name_ptrs = functions + 4 * n;
        let ordinals = name_ptrs + 4 * n;
        let mut strings = ordinals + 2 * n;
        let dll_name_rva = strings;
        strings += dll_name.len() as u32 + 1;
        let mut d = Vec::new();
        for v in [0u32, 0, 0, dll_name_rva, 1, n, n, functions, name_ptrs, ordinals] {
            // Characteristics, TimeDateStamp, Major/MinorVersion (as one u32), Name, Base, counts, tables.
            d.extend_from_slice(&v.to_le_bytes());
        }
        for (_, r) in &names {
            d.extend_from_slice(&r.to_le_bytes());
        }
        let mut name_rva = strings;
        for (s, _) in &names {
            d.extend_from_slice(&name_rva.to_le_bytes());
            name_rva += s.len() as u32 + 1;
        }
        for i in 0..n as u16 {
            d.extend_from_slice(&i.to_le_bytes());
        }
        d.extend_from_slice(dll_name.as_bytes());
        d.push(0);
        for (s, _) in &names {
            d.extend_from_slice(s.as_bytes());
            d.push(0);
        }
        export_dir = (base, d.len() as u32);
        let size = d.len() as u32;
        outs.push(Out { name: short_name(".edata"), data: d, virtual_size: size, characteristics: RDATA, rva: base, uninitialized: false });
        rva = align(rva + size, SECTION_ALIGN);
    }

    // Base relocations, one block per 4 KiB page.
    base_relocs.sort_unstable();
    let mut relocs = Vec::new();
    let mut i = 0;
    while i < base_relocs.len() {
        let page = base_relocs[i] & !0xFFF;
        let mut entries: Vec<u16> = Vec::new();
        while i < base_relocs.len() && base_relocs[i] & !0xFFF == page {
            entries.push(0xA000 | (base_relocs[i] & 0xFFF) as u16); // IMAGE_REL_BASED_DIR64
            i += 1;
        }
        if entries.len() % 2 == 1 {
            entries.push(0); // IMAGE_REL_BASED_ABSOLUTE padding
        }
        relocs.extend_from_slice(&page.to_le_bytes());
        relocs.extend_from_slice(&(8 + 2 * entries.len() as u32).to_le_bytes());
        for e in entries {
            relocs.extend_from_slice(&e.to_le_bytes());
        }
    }
    let reloc_dir = (rva, relocs.len() as u32);
    let reloc_size = relocs.len().max(1) as u32;
    outs.push(Out { name: short_name(".reloc"), data: relocs, virtual_size: reloc_size, characteristics: RELOC, rva, uninitialized: false });
    rva = align(rva + reloc_size, SECTION_ALIGN);
    let size_of_image = rva;

    // File layout.
    let mut file_offset = size_of_headers;
    let mut placement = Vec::new();
    for o in &outs {
        let raw = if o.uninitialized { 0 } else { align(o.data.len() as u32, FILE_ALIGN) };
        placement.push((if raw == 0 { 0 } else { file_offset }, raw));
        file_offset += raw;
    }

    let mut f = vec![0u8; size_of_headers as usize];
    // DOS header: only e_magic and e_lfanew are needed.
    f[0..2].copy_from_slice(b"MZ");
    f[0x3C..0x40].copy_from_slice(&0x40u32.to_le_bytes());
    let mut h = Vec::new();
    h.extend_from_slice(b"PE\0\0");
    // COFF header.
    h.extend_from_slice(&0x8664u16.to_le_bytes()); // AMD64
    h.extend_from_slice(&(outs.len() as u16).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes()); // TimeDateStamp: 0 keeps the output deterministic
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes());
    h.extend_from_slice(&240u16.to_le_bytes());
    h.extend_from_slice(&(0x0002u16 | 0x0020 | 0x2000).to_le_bytes()); // EXECUTABLE_IMAGE | LARGE_ADDRESS_AWARE | DLL
    // Optional header (PE32+).
    let sum = |ch: u32| -> u32 {
        outs.iter().zip(&placement).filter(|(o, _)| o.characteristics == ch).map(|(o, p)| if o.uninitialized { align(o.virtual_size, FILE_ALIGN) } else { p.1 }).sum()
    };
    let text_rva = outs.iter().find(|o| o.characteristics == CODE).map(|o| o.rva).unwrap_or(0);
    h.extend_from_slice(&0x20Bu16.to_le_bytes());
    h.extend_from_slice(&[14, 0]); // linker version
    h.extend_from_slice(&sum(CODE).to_le_bytes());
    h.extend_from_slice(&(sum(RDATA) + sum(DATA) + sum(RELOC)).to_le_bytes());
    h.extend_from_slice(&sum(BSS).to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes()); // AddressOfEntryPoint: none
    h.extend_from_slice(&text_rva.to_le_bytes());
    h.extend_from_slice(&IMAGE_BASE.to_le_bytes());
    h.extend_from_slice(&SECTION_ALIGN.to_le_bytes());
    h.extend_from_slice(&FILE_ALIGN.to_le_bytes());
    for v in [6u16, 0, 0, 0, 6, 0] {
        h.extend_from_slice(&v.to_le_bytes()); // OS, image and subsystem versions
    }
    h.extend_from_slice(&0u32.to_le_bytes()); // Win32VersionValue
    h.extend_from_slice(&size_of_image.to_le_bytes());
    h.extend_from_slice(&size_of_headers.to_le_bytes());
    h.extend_from_slice(&0u32.to_le_bytes()); // CheckSum
    h.extend_from_slice(&2u16.to_le_bytes()); // Subsystem: Windows GUI
    h.extend_from_slice(&(0x0020u16 | 0x0040 | 0x0100).to_le_bytes()); // HIGH_ENTROPY_VA | DYNAMIC_BASE | NX_COMPAT
    for v in [0x10_0000u64, 0x1000, 0x10_0000, 0x1000] {
        h.extend_from_slice(&v.to_le_bytes()); // stack and heap reserve/commit
    }
    h.extend_from_slice(&0u32.to_le_bytes()); // LoaderFlags
    h.extend_from_slice(&16u32.to_le_bytes());
    let pdata_dir = pdata.map(|p| (outs[p].rva, outs[p].data.len() as u32)).unwrap_or((0, 0));
    let mut dirs = [(0u32, 0u32); 16];
    dirs[0] = export_dir;
    dirs[3] = pdata_dir;
    dirs[5] = reloc_dir;
    for (r, s) in dirs {
        h.extend_from_slice(&r.to_le_bytes());
        h.extend_from_slice(&s.to_le_bytes());
    }
    // Section headers.
    for (o, (ptr, raw)) in outs.iter().zip(&placement) {
        h.extend_from_slice(&o.name);
        h.extend_from_slice(&o.virtual_size.to_le_bytes());
        h.extend_from_slice(&o.rva.to_le_bytes());
        h.extend_from_slice(&raw.to_le_bytes());
        h.extend_from_slice(&ptr.to_le_bytes());
        h.extend_from_slice(&[0u8; 12]); // relocations, line numbers and their counts
        h.extend_from_slice(&o.characteristics.to_le_bytes());
    }
    f[0x40..0x40 + h.len()].copy_from_slice(&h);
    for (o, (_, raw)) in outs.iter().zip(&placement) {
        if *raw == 0 {
            continue;
        }
        let mut d = o.data.clone();
        d.resize(*raw as usize, 0);
        f.extend_from_slice(&d);
    }
    Ok(f)
}
