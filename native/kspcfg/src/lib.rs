//! Byte-exact replacement for the text half of KSP 1.12.5's `ConfigNode.Load(string)`.
//!
//! Stock KSP reads a cfg with `File.ReadAllLines`, runs `PreFormatConfig` (strip `//` comments, trim, split
//! braces onto their own lines) and `CustomEqualSplit` (split on the first `=`), then builds the tree with
//! `RecurseFormat`. The text half is ~75% of the cost, and `PreFormatConfig` does O(n^2) `List.Insert`s.
//! This crate does the text half and hands back UTF-16 text plus token offsets; the managed side keeps stock
//! `RecurseFormat` and the stock `ConfigNode` constructors, so node naming, `CleanupInput` and id splitting
//! stay exactly stock.
//!
//! Stock finds `//`, `{` and `}` with culture-sensitive `string.IndexOf`, which can treat some characters next
//! to them differently from an ordinal search. Only text in an allowlist of known-safe characters takes the
//! fast path; anything else, invalid UTF-8, or a UTF-16/32 byte-order mark is reported as `STATUS_FALLBACK` so
//! the caller uses the stock parser for that file.

mod fastpath_exclusions;

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

pub const STATUS_OK: i32 = 0;
/// No lines at all: stock logs "Error: Empty part config file" and returns null; caller should defer to stock.
pub const STATUS_EMPTY_FILE: i32 = 1;
/// Content whose stock behaviour depends on culture-sensitive search or decoding; caller should use stock.
pub const STATUS_FALLBACK: i32 = 2;
/// The file could not be read; caller should use stock (which logs its own warning).
pub const STATUS_IO_ERROR: i32 = 3;
/// kspcfg itself failed (a bug); caller should use stock.
pub const STATUS_INTERNAL_ERROR: i32 = 4;

fn guarded(f: impl FnOnce() -> Parsed) -> Parsed {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| Parsed::status(STATUS_INTERNAL_ERROR))
}

/// Paths arrive from C# as UTF-16 on every platform. Windows paths are UTF-16 natively, so they convert
/// losslessly; elsewhere they are decoded to UTF-8, which is what the file system expects.
#[cfg(windows)]
fn path_from_utf16(units: &[u16]) -> PathBuf {
    use std::os::windows::ffi::OsStringExt;
    PathBuf::from(std::ffi::OsString::from_wide(units))
}

#[cfg(not(windows))]
fn path_from_utf16(units: &[u16]) -> PathBuf {
    PathBuf::from(String::from_utf16_lossy(units))
}

unsafe fn wide_path(ptr: *const u16, len: usize) -> PathBuf {
    path_from_utf16(std::slice::from_raw_parts(ptr, len))
}

/// One preformatted line: one string (a name or brace) or a name/value pair. Offsets are UTF-16 code units
/// into `text16`, so the managed side can create each string with a plain copy.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Token {
    pub count: u32,
    pub a_start: u32,
    pub a_len: u32,
    pub b_start: u32,
    pub b_len: u32,
}

pub struct Parsed {
    pub status: i32,
    pub text16: Vec<u16>,
    pub tokens: Vec<Token>,
}

impl Parsed {
    fn status(status: i32) -> Parsed {
        Parsed { status, text16: Vec::new(), tokens: Vec::new() }
    }

    /// The token's strings, for tests and tools.
    pub fn strings(&self, t: &Token) -> Vec<String> {
        let s = |start: u32, len: u32| String::from_utf16(&self.text16[start as usize..(start + len) as usize]).unwrap();
        if t.count == 1 { vec![s(t.a_start, t.a_len)] } else { vec![s(t.a_start, t.a_len), s(t.b_start, t.b_len)] }
    }
}

/// `char.IsWhiteSpace`, which `string.Trim()` uses on .NET and Mono. U+180E is not on the fast path at all,
/// because its classification changed between Unicode versions.
fn is_dotnet_whitespace(c: char) -> bool {
    matches!(c,
        '\u{0009}'..='\u{000D}' | '\u{0020}' | '\u{0085}' | '\u{00A0}' | '\u{1680}' |
        '\u{2000}'..='\u{200A}' | '\u{2028}' | '\u{2029}' | '\u{202F}' | '\u{205F}' | '\u{3000}')
}

/// Non-ASCII characters allowed on the fast path. Stock finds `//`, `{` and `}` with culture-sensitive IndexOf,
/// which ignores code points its collation tables do not know and treats repeat/sound marks specially. The ranges
/// below are characters assigned by Unicode 3.0 in the scripts KSP configs use, without mark-like characters;
/// `fastpath_exclusions` removes the ones KSP's Mono still treats specially (found by running the allowlist test
/// inside KSP's own Mono with tools/monohost). Anything else sends the file to the stock parser, which only costs
/// speed.
fn fast_path_char(c: char) -> bool {
    if c.is_ascii() {
        return true;
    }
    in_fast_path_ranges(c as u32) && !excluded(c as u32)
}

fn excluded(cp: u32) -> bool {
    fastpath_exclusions::EXCLUDED
        .binary_search_by(|&(lo, hi)| {
            if hi < cp {
                std::cmp::Ordering::Less
            } else if lo > cp {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

fn in_fast_path_ranges(cp: u32) -> bool {
    matches!(cp,
        0x00A0..=0x00AC | 0x00AE..=0x0220 | 0x0222..=0x0233 |   // Latin-1 (minus soft hyphen), Latin Extended-A/B
        0x0386 | 0x0388..=0x038A | 0x038C | 0x038E..=0x03A1 | 0x03A3..=0x03CE | // Greek letters
        0x0400..=0x0482 | 0x048C..=0x04C4 | 0x04C7..=0x04C8 | 0x04CB..=0x04CC | 0x04D0..=0x04F5 | 0x04F8..=0x04F9 | // Cyrillic
        0x2000..=0x200A | 0x2010..=0x2029 | 0x202F | 0x2030..=0x2046 | 0x2048..=0x204D | // spaces, punctuation
        0x2070 | 0x2074..=0x208E |                               // superscripts, subscripts
        0x20A0..=0x20AF |                                        // currency
        0x2100..=0x2138 | 0x2153..=0x2183 | 0x2190..=0x21F3 |    // letterlike, number forms, arrows
        0x2200..=0x22F1 | 0x2300..=0x237B | 0x237D..=0x239A |    // math, technical
        0x2460..=0x24EA | 0x2500..=0x2595 | 0x25A0..=0x25F7 | 0x2600..=0x2613 | // enclosed, box, shapes, symbols
        0x3000..=0x3029 | 0x3030 | 0x3036..=0x303A | 0x303F |    // CJK punctuation (no combining or repeat marks)
        0x3041..=0x3094 | 0x30A1..=0x30FB |                      // hiragana, katakana (no sound/iteration marks)
        0x3400..=0x4DB5 | 0x4E00..=0x9FA5 |                      // CJK ideographs
        0xAC00..=0xD7A3 |                                        // Hangul syllables
        0xFF01..=0xFF5E | 0xFF61..=0xFF6F | 0xFF71..=0xFF9D |    // fullwidth / halfwidth (no FF70, FF9E-FF9F)
        0xFFE0..=0xFFE6 | 0xFFE8..=0xFFEE)
}

fn trim(text: &str, mut start: usize, mut end: usize) -> (usize, usize) {
    while start < end {
        let c = text[start..end].chars().next().unwrap();
        if !is_dotnet_whitespace(c) {
            break;
        }
        start += c.len_utf8();
    }
    while end > start {
        let c = text[start..end].chars().next_back().unwrap();
        if !is_dotnet_whitespace(c) {
            break;
        }
        end -= c.len_utf8();
    }
    (start, end)
}

fn find_byte(text: &[u8], start: usize, end: usize, b: u8) -> Option<usize> {
    text[start..end].iter().position(|&x| x == b).map(|i| start + i)
}

fn find_comment(text: &[u8], start: usize, end: usize) -> Option<usize> {
    text[start..end].windows(2).position(|w| w == b"//").map(|i| start + i)
}

enum Work {
    Range(usize, usize),
    Brace(usize),
}

/// Stock PreFormatConfig for one source line, equivalent to its backwards in-place insert loop: a piece with a
/// `}` splits at the first `}` (else at the first `{`) into prefix, brace, suffix, and each piece is processed
/// again from the top. Output lines are appended in document order.
fn preformat_line(text: &str, start: usize, end: usize, out: &mut Vec<(usize, usize)>, stack: &mut Vec<Work>) {
    let bytes = text.as_bytes();
    stack.clear();
    stack.push(Work::Range(start, end));
    while let Some(w) = stack.pop() {
        let (s, e) = match w {
            Work::Brace(at) => {
                out.push((at, at + 1));
                continue;
            }
            Work::Range(s, e) => (s, e),
        };
        let mut e = e;
        if let Some(c) = find_comment(bytes, s, e) {
            if c == s {
                continue;
            }
            e = c;
        }
        let (s, e) = trim(text, s, e);
        if s == e {
            continue;
        }
        let brace = find_byte(bytes, s, e, b'}').or_else(|| find_byte(bytes, s, e, b'{'));
        match brace {
            Some(p) if !(p == s && e - s == 1) => {
                // Pushed in reverse so they pop in document order: prefix, brace, suffix.
                if p + 1 < e {
                    stack.push(Work::Range(p + 1, e));
                }
                stack.push(Work::Brace(p));
                if p > s {
                    stack.push(Work::Range(s, p));
                }
            }
            _ => out.push((s, e)),
        }
    }
}

/// Stock CustomEqualSplit on an already trimmed line.
fn equal_split(text: &str, s: usize, e: usize) -> Token {
    let bytes = text.as_bytes();
    match find_byte(bytes, s, e, b'=') {
        None => Token { count: 1, a_start: s as u32, a_len: (e - s) as u32, ..Token::default() },
        Some(i) => {
            let (a0, a1) = if i == s { (s, s) } else { trim(text, s, i) };
            let (b0, b1) = if i + 1 >= e { (e, e) } else { trim(text, i + 1, e) };
            Token {
                count: 2,
                a_start: a0 as u32,
                a_len: (a1 - a0) as u32,
                b_start: b0 as u32,
                b_len: (b1 - b0) as u32,
            }
        }
    }
}

/// Parse a cfg file's raw bytes the way `ConfigNode.Load` would see them through `File.ReadAllLines`.
pub fn parse_bytes(raw: &[u8]) -> Parsed {
    if raw.len() > (u32::MAX / 4) as usize {
        return Parsed::status(STATUS_FALLBACK);
    }
    if raw.starts_with(&[0xFF, 0xFE]) || raw.starts_with(&[0xFE, 0xFF]) {
        return Parsed::status(STATUS_FALLBACK); // UTF-16/32 BOM: StreamReader would switch encodings.
    }
    let body = raw.strip_prefix(&[0xEF, 0xBB, 0xBF][..]).unwrap_or(raw);
    let text = match std::str::from_utf8(body) {
        Ok(t) => t,
        Err(_) => return Parsed::status(STATUS_FALLBACK), // Replacement-character rules differ by runtime.
    };
    if !text.chars().all(fast_path_char) {
        return Parsed::status(STATUS_FALLBACK);
    }

    let bytes = text.as_bytes();
    let mut lines = 0usize;
    let mut pieces: Vec<(usize, usize)> = Vec::with_capacity(bytes.len() / 24 + 8);
    let mut stack = Vec::with_capacity(16);
    let mut i = 0;
    let mut line_start = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\n' => {
                lines += 1;
                preformat_line(text, line_start, i, &mut pieces, &mut stack);
                i += 1;
                line_start = i;
            }
            b'\r' => {
                lines += 1;
                preformat_line(text, line_start, i, &mut pieces, &mut stack);
                i += if bytes.get(i + 1) == Some(&b'\n') { 2 } else { 1 };
                line_start = i;
            }
            _ => i += 1,
        }
    }
    if line_start < bytes.len() {
        lines += 1;
        preformat_line(text, line_start, bytes.len(), &mut pieces, &mut stack);
    }
    if lines == 0 {
        return Parsed::status(STATUS_EMPTY_FILE);
    }
    let mut tokens: Vec<Token> = pieces.iter().map(|&(s, e)| equal_split(text, s, e)).collect();
    let text16: Vec<u16> = text.encode_utf16().collect();
    if !text.is_ascii() {
        // Byte offsets -> UTF-16 offsets. Every token boundary is a char boundary.
        let mut map = vec![0u32; text.len() + 1];
        let mut u = 0u32;
        for (b, c) in text.char_indices() {
            map[b] = u;
            u += c.len_utf16() as u32;
        }
        map[text.len()] = u;
        for t in &mut tokens {
            let (a0, a1) = (map[t.a_start as usize], map[(t.a_start + t.a_len) as usize]);
            let (b0, b1) = (map[t.b_start as usize], map[(t.b_start + t.b_len) as usize]);
            *t = Token { count: t.count, a_start: a0, a_len: a1 - a0, b_start: b0, b_len: b1 - b0 };
        }
    }
    Parsed { status: STATUS_OK, text16, tokens }
}

pub fn parse_file(path: &std::path::Path) -> Parsed {
    match std::fs::read(path) {
        Ok(raw) => parse_bytes(&raw),
        Err(_) => Parsed::status(STATUS_IO_ERROR),
    }
}

/// Parse many files on `threads` worker threads; results keep the input order.
pub fn parse_many(paths: &[std::path::PathBuf], threads: usize) -> Vec<Parsed> {
    let next = AtomicUsize::new(0);
    let mut slots: Vec<Option<Parsed>> = (0..paths.len()).map(|_| None).collect();
    let chunks: Vec<std::sync::Mutex<Vec<(usize, Parsed)>>> =
        (0..threads.max(1)).map(|_| std::sync::Mutex::new(Vec::new())).collect();
    std::thread::scope(|scope| {
        for chunk in &chunks {
            let next = &next;
            scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= paths.len() {
                        break;
                    }
                    local.push((i, guarded(|| parse_file(&paths[i]))));
                }
                *chunk.lock().unwrap() = local;
            });
        }
    });
    for chunk in chunks {
        for (i, p) in chunk.into_inner().unwrap() {
            slots[i] = Some(p);
        }
    }
    slots.into_iter().map(|p| p.unwrap()).collect()
}

// ---------------------------------------------------------------------------------------------------------
// C ABI for the managed shim. Every `Parsed` returned must be released with `kspcfg_free`.

pub struct Batch {
    items: Vec<Parsed>,
}

#[no_mangle]
pub extern "C" fn kspcfg_version() -> u32 {
    6
}

/// `path` is UTF-16 (what C# strings are), `len` in UTF-16 code units.
#[no_mangle]
pub unsafe extern "C" fn kspcfg_parse_file(path: *const u16, len: usize) -> *mut Parsed {
    let p = wide_path(path, len);
    Box::into_raw(Box::new(guarded(|| parse_file(&p))))
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_parse_bytes(data: *const u8, len: usize) -> *mut Parsed {
    let bytes = std::slice::from_raw_parts(data, len);
    Box::into_raw(Box::new(guarded(|| parse_bytes(bytes))))
}

/// `paths` is UTF-16 text with paths separated by NUL; returns a batch indexed in the same order.
#[no_mangle]
pub unsafe extern "C" fn kspcfg_parse_many(paths: *const u16, len: usize, threads: u32) -> *mut Batch {
    let all = std::slice::from_raw_parts(paths, len);
    let list: Vec<PathBuf> = all.split(|&c| c == 0).filter(|s| !s.is_empty())
        .map(path_from_utf16).collect();
    let items = catch_unwind(AssertUnwindSafe(|| parse_many(&list, threads as usize)))
        .unwrap_or_else(|_| list.iter().map(|_| Parsed::status(STATUS_INTERNAL_ERROR)).collect());
    Box::into_raw(Box::new(Batch { items }))
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_batch_len(b: *const Batch) -> usize {
    if b.is_null() { 0 } else { (*b).items.len() }
}

/// Borrowed pointer into the batch, valid until `kspcfg_batch_free`; null for a null batch or bad index.
#[no_mangle]
pub unsafe extern "C" fn kspcfg_batch_get(b: *const Batch, i: usize) -> *const Parsed {
    if b.is_null() {
        return std::ptr::null();
    }
    let items = &(*b).items;
    items.get(i).map_or(std::ptr::null(), |p| p as *const Parsed)
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_batch_free(b: *mut Batch) {
    if !b.is_null() {
        drop(Box::from_raw(b));
    }
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_status(p: *const Parsed) -> i32 {
    if p.is_null() { STATUS_INTERNAL_ERROR } else { (*p).status }
}

#[no_mangle]
/// UTF-16 text the token offsets point into.
pub unsafe extern "C" fn kspcfg_text(p: *const Parsed, out: *mut *const u16) -> usize {
    *out = (*p).text16.as_ptr();
    (*p).text16.len()
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_tokens(p: *const Parsed, out: *mut *const Token) -> usize {
    *out = (*p).tokens.as_ptr();
    (*p).tokens.len()
}

#[no_mangle]
pub unsafe extern "C" fn kspcfg_free(p: *mut Parsed) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(src: &str) -> Vec<Vec<String>> {
        let p = parse_bytes(src.as_bytes());
        assert_eq!(p.status, STATUS_OK);
        p.tokens.iter().map(|t| p.strings(t)).collect()
    }

    fn v(items: &[&[&str]]) -> Vec<Vec<String>> {
        items.iter().map(|l| l.iter().map(|s| s.to_string()).collect()).collect()
    }

    #[test]
    fn braces_on_one_line_split_in_document_order() {
        assert_eq!(lines("PART { name = a }"), v(&[&["PART"], &["{"], &["name", "a"], &["}"]]));
        assert_eq!(lines("a{b}c"), v(&[&["a"], &["{"], &["b"], &["}"], &["c"]]));
        assert_eq!(lines("}}{{"), v(&[&["}"], &["}"], &["{"], &["{"]]));
    }

    #[test]
    fn comments_whitespace_and_equals() {
        assert_eq!(lines("  // whole line\n\tkey = v // tail\n"), v(&[&["key", "v"]]));
        assert_eq!(lines("=x\ny=\n=\na = b = c"), v(&[&["", "x"], &["y", ""], &["", ""], &["a", "b = c"]]));
        assert_eq!(lines("\u{00A0}name\u{3000}=\u{2003}value\u{00A0}"), v(&[&["name", "value"]]));
    }

    #[test]
    fn non_ascii_offsets_are_utf16() {
        assert_eq!(lines("имя = значение { 漢字 = √x }"),
            v(&[&["имя", "значение"], &["{"], &["漢字", "√x"], &["}"]]));
    }

    #[test]
    fn line_endings_and_bom() {
        assert_eq!(lines("\u{FEFF}a\r\nb\rc\nd"), v(&[&["a"], &["b"], &["c"], &["d"]]));
        assert_eq!(parse_bytes(b"").status, STATUS_EMPTY_FILE);
        assert_eq!(parse_bytes("\u{FEFF}".as_bytes()).status, STATUS_EMPTY_FILE);
        assert_eq!(parse_bytes(b"// only a comment").tokens.len(), 0);
    }

    #[test]
    fn culture_sensitive_inputs_fall_back() {
        assert_eq!(parse_bytes("a\u{200B}//b".as_bytes()).status, STATUS_FALLBACK);
        assert_eq!(parse_bytes("a}\u{0301}".as_bytes()).status, STATUS_FALLBACK);
        assert_eq!(parse_bytes(&[0x61, 0xFF, 0x62]).status, STATUS_FALLBACK);
        assert_eq!(parse_bytes(&[0xFF, 0xFE, 0x61, 0x00]).status, STATUS_FALLBACK);
        assert_eq!(parse_bytes("a\u{0600}b".as_bytes()).status, STATUS_FALLBACK); // Arabic number sign (Cf)
        assert_eq!(parse_bytes("a\u{1E000}b".as_bytes()).status, STATUS_FALLBACK); // supplementary combining mark
        assert_eq!(parse_bytes("a\u{0E01}b".as_bytes()).status, STATUS_FALLBACK); // unlisted script
        assert_eq!(parse_bytes("a/\u{0378}/b".as_bytes()).status, STATUS_FALLBACK); // unassigned in Greek block
        assert_eq!(parse_bytes("\u{FF9E}{".as_bytes()).status, STATUS_FALLBACK); // halfwidth voiced sound mark
        assert_eq!(parse_bytes("\u{30FC}}".as_bytes()).status, STATUS_FALLBACK); // prolonged sound mark
        assert_eq!(parse_bytes("a/\u{0400}/b".as_bytes()).status, STATUS_FALLBACK); // unknown to Mono's tables
    }

    #[test]
    fn exclusion_table_is_sorted_and_disjoint() {
        let t = fastpath_exclusions::EXCLUDED;
        assert!(t.iter().all(|&(lo, hi)| lo <= hi));
        assert!(t.windows(2).all(|w| w[0].1 < w[1].0));
        assert!(excluded(0x0400) && excluded(0x0222) && !excluded(0x0401) && !excluded(0x0041));
    }
}
