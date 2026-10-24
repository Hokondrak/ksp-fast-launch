//! Bounds-checked ECMA-335 metadata reader. No code from the inspected assembly runs.
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Assembly {
    pub name: String,
    pub refs: Vec<String>,
    pub methods: u32,
    pub mvid: String,
}
fn range(b: &[u8], p: usize, n: usize) -> Result<&[u8], String> {
    b.get(p..p.checked_add(n).ok_or("offset overflow")?)
        .ok_or("truncated PE/metadata".into())
}
fn num(b: &[u8], p: usize, n: usize) -> Result<u32, String> {
    let mut v = 0u32;
    for (i, x) in range(b, p, n)?.iter().enumerate() {
        v |= (*x as u32) << (i * 8);
    }
    Ok(v)
}
fn zstr(b: &[u8], p: usize) -> Result<String, String> {
    let tail = b.get(p..).ok_or("bad string offset")?;
    let n = tail
        .iter()
        .position(|x| *x == 0)
        .ok_or("unterminated metadata string")?;
    String::from_utf8(tail[..n].to_vec()).map_err(|_| "non-UTF8 metadata string".into())
}

pub fn read(path: &Path) -> Result<Option<Assembly>, String> {
    let b = fs::read(path).map_err(|e| e.to_string())?;
    parse(&b)
}

/// Check the actual PE export table, not a string that could merely occur in debug data.
pub fn validate_aot(b: &[u8]) -> Result<(), String> {
    if range(b, 0, 2)? != b"MZ" {
        return Err("AOT image is not PE".into());
    }
    let pe = num(b, 0x3c, 4)? as usize;
    if range(b, pe, 4)? != b"PE\0\0"
        || num(b, pe + 4, 2)? != 0x8664
        || num(b, pe + 22, 2)? & 0x2000 == 0
    {
        return Err("AOT image is not an AMD64 DLL".into());
    }
    let opt = pe + 24;
    let sec = opt + num(b, pe + 20, 2)? as usize;
    let n = num(b, pe + 6, 2)? as usize;
    if num(b, opt, 2)? != 0x20b || num(b, opt + 16, 4)? != 0 {
        return Err("AOT DLL has wrong format or an entry point".into());
    }
    let rva = |addr: u32| -> Result<usize, String> {
        for i in 0..n {
            let p = sec + i * 40;
            let base = num(b, p + 12, 4)?;
            let size = num(b, p + 16, 4)?;
            if addr >= base && addr - base < size {
                return Ok(num(b, p + 20, 4)? as usize + (addr - base) as usize);
            }
        }
        Err("invalid AOT RVA".into())
    };
    let dir = opt + 112;
    if num(b, dir + 8, 4)? != 0 || num(b, dir + 9 * 8, 4)? != 0 {
        return Err("AOT DLL unexpectedly has imports or TLS callbacks".into());
    }
    let export = num(b, dir, 4)?;
    let export_size = num(b, dir + 4, 4)?;
    let e = rva(export)?;
    let count = num(b, e + 24, 4)? as usize;
    let names = rva(num(b, e + 32, 4)?)?;
    let ordinals = rva(num(b, e + 36, 4)?)?;
    let functions = rva(num(b, e + 28, 4)?)?;
    range(
        b,
        names,
        count.checked_mul(4).ok_or("export count overflow")?,
    )?;
    for i in 0..count {
        if zstr(b, rva(num(b, names + i * 4, 4)?)?)? == "mono_aot_file_info" {
            let ordinal = num(b, ordinals + i * 2, 2)?;
            if ordinal >= num(b, e + 20, 4)? {
                return Err("invalid export ordinal".into());
            }
            let target = num(b, functions + ordinal as usize * 4, 4)?;
            if target >= export && target - export < export_size {
                return Err("forwarded AOT export is unsupported".into());
            }
            range(b, rva(target)?, 160)?;
            return Ok(());
        }
    }
    Err("mono_aot_file_info export is missing".into())
}
pub fn parse(b: &[u8]) -> Result<Option<Assembly>, String> {
    if b.get(..2) != Some(b"MZ") {
        return Ok(None);
    }
    let pe = num(b, 0x3c, 4)? as usize;
    if range(b, pe, 4)? != b"PE\0\0" {
        return Err("invalid PE signature".into());
    }
    let sections = num(b, pe + 6, 2)? as usize;
    let optional_size = num(b, pe + 20, 2)? as usize;
    let opt = pe + 24;
    let dir = match num(b, opt, 2)? {
        0x10b => opt + 96,
        0x20b => opt + 112,
        _ => return Ok(None),
    };
    if dir + 15 * 8 > opt + optional_size {
        return Ok(None);
    }
    let cli_rva = num(b, dir + 14 * 8, 4)?;
    if cli_rva == 0 {
        return Ok(None);
    }
    let sec = opt + optional_size;
    let rva = |addr: u32| -> Result<usize, String> {
        for i in 0..sections {
            let s = sec + i * 40;
            let base = num(b, s + 12, 4)?;
            let size = num(b, s + 16, 4)?;
            if addr >= base && addr - base < size {
                return Ok(num(b, s + 20, 4)? as usize + (addr - base) as usize);
            }
        }
        Err(format!("unmapped RVA {addr:x}"))
    };
    let cli = rva(cli_rva)?;
    let root = rva(num(b, cli + 8, 4)?)?;
    if num(b, root, 4)? != 0x424a5342 {
        return Err("invalid CLI metadata signature".into());
    }
    let mut p = root + 16 + num(b, root + 12, 4)? as usize;
    p = (p + 3) & !3;
    let streams = num(b, p + 2, 2)? as usize;
    p += 4;
    let (mut table, mut strings, mut guids) = (None, None, None);
    for _ in 0..streams {
        let off = num(b, p, 4)? as usize;
        let size = num(b, p + 4, 4)? as usize;
        let name = zstr(b, p + 8)?;
        let data = range(b, root + off, size)?;
        match name.as_str() {
            "#~" | "#-" => table = Some(data),
            "#Strings" => strings = Some(data),
            "#GUID" => guids = Some(data),
            _ => {}
        }
        p = (p + 8 + name.len() + 1 + 3) & !3;
    }
    let t = table.ok_or("no metadata tables")?;
    let strings = strings.ok_or("no strings heap")?;
    let guids = guids.ok_or("no GUID heap")?;
    let heap = num(t, 6, 1)?;
    let mut rows = [0u32; 64];
    let valid = (num(t, 12, 4)? as u64) << 32 | num(t, 8, 4)? as u64;
    p = 24;
    for (i, row) in rows.iter_mut().enumerate() {
        if valid & (1u64 << i) != 0 {
            *row = num(t, p, 4)?;
            p += 4;
        }
    }
    let s = if heap & 1 != 0 { 4 } else { 2 };
    let g = if heap & 2 != 0 { 4 } else { 2 };
    let blob = if heap & 4 != 0 { 4 } else { 2 };
    let ix = |i: usize| if rows[i] >= 65536 { 4 } else { 2 };
    let code = |bits: u32, ids: &[usize]| {
        if ids.iter().any(|i| rows[*i] >= (1u32 << (16 - bits))) {
            4
        } else {
            2
        }
    };
    let td = code(2, &[2, 1, 27]);
    let resolution = code(2, &[0, 26, 35, 1]);
    let hasconst = code(2, &[4, 8, 23]);
    let hasattr = code(
        5,
        &[
            6, 4, 1, 2, 8, 9, 10, 0, 14, 23, 20, 17, 26, 27, 32, 35, 38, 39, 40, 42, 44, 43,
        ],
    );
    let attrtype = code(3, &[6, 10]);
    let memberparent = code(3, &[2, 1, 26, 6, 27]);
    let sizes = [
        2 + s + 3 * g,
        resolution + 2 * s,
        4 + 2 * s + td + ix(4) + ix(6),
        ix(4),
        2 + s + blob,
        ix(6),
        8 + s + blob + ix(8),
        ix(8),
        4 + s,
        ix(2) + td,
        memberparent + s + blob,
        2 + hasconst + blob,
        hasattr + attrtype + blob,
        code(1, &[4, 8]) + blob,
        2 + code(2, &[2, 6, 32]) + blob,
        6 + ix(2),
        4 + ix(4),
        blob,
        ix(2) + ix(20),
        ix(20),
        2 + s + td,
        ix(2) + ix(23),
        ix(23),
        2 + s + blob,
        2 + ix(6) + code(1, &[20, 23]),
        ix(2) + 2 * code(1, &[6, 10]),
        s,
        blob,
        2 + code(1, &[4, 6]) + s + ix(26),
        4 + ix(4),
        8,
        4,
        16 + blob + 2 * s,
        4,
        12,
        12 + 2 * blob + 2 * s,
    ];
    let mut starts = [0usize; 36];
    for i in 0..36 {
        starts[i] = p;
        let size = (rows[i] as usize)
            .checked_mul(sizes[i])
            .ok_or("table size overflow")?;
        range(t, p, size)?;
        p = p.checked_add(size).ok_or("table offset overflow")?;
    }
    if rows[32] == 0 {
        return Ok(None);
    } // netmodule, no standalone assembly
    if rows[32] != 1 || rows[0] != 1 {
        return Err("invalid assembly/module row count".into());
    }
    let name = zstr(strings, num(t, starts[32] + 16 + blob, s)? as usize)?;
    if name.is_empty()
        || name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '\0' | '\t' | '\n' | '\r'))
    {
        return Err("unsafe assembly name".into());
    }
    let guidindex = num(t, starts[0] + 2 + s, g)? as usize;
    if guidindex == 0 {
        return Err("missing MVID".into());
    }
    let mvid = range(guids, (guidindex - 1) * 16, 16)?
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let mut refs = Vec::new();
    for i in 0..rows[35] as usize {
        refs.push(zstr(
            strings,
            num(t, starts[35] + i * sizes[35] + 12 + blob, s)? as usize,
        )?);
    }
    Ok(Some(Assembly {
        name,
        refs,
        methods: rows[6],
        mvid,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_truncated_pe_without_panicking() {
        assert!(parse(b"MZ").is_err());
        assert!(parse(b"not managed").unwrap().is_none());
        for n in 2..512 {
            let mut b = vec![0; n];
            b[..2].copy_from_slice(b"MZ");
            let _ = parse(&b);
        }
    }
}
