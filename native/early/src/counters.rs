//! Mono's own runtime counters (JIT time, methods compiled, classes and images loaded, GC), read through the
//! embedding API Unity's Mono exports. They must be enabled before the runtime initialises, which is exactly when
//! the loader's `mono_jit_init_version` wrapper runs, and are read at the main menu.
//!
//! The point is measurement: how much of a boot is spent compiling managed code decides whether caching compiled
//! code (AOT) is worth pursuing.

use crate::win;
use std::ffi::{c_char, c_void, CStr};

type Counter = *mut c_void;
type ForeachCallback = unsafe extern "C" fn(Counter, *mut c_void) -> i32;

// mono/utils/mono-counters.h
const TYPE_MASK: i32 = 0xf;
const INT: i32 = 0;
const UINT: i32 = 1;
const WORD: i32 = 2;
const LONG: i32 = 3;
const ULONG: i32 = 4;
const DOUBLE: i32 = 5;
const STRING: i32 = 6;
const TIME_INTERVAL: i32 = 7;
/// Every section (JIT, GC, metadata, generics, security, runtime, system, profiler).
const ALL_SECTIONS: i32 = -1;

struct Api {
    foreach: unsafe extern "C" fn(ForeachCallback, *mut c_void),
    name: unsafe extern "C" fn(Counter) -> *const c_char,
    kind: unsafe extern "C" fn(Counter) -> i32,
    section: unsafe extern "C" fn(Counter) -> i32,
    sample: unsafe extern "C" fn(Counter, *mut c_void, i32) -> i32,
}

unsafe fn get(mono: win::Hmodule, gpa: unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void, name: &CStr) -> Option<*mut c_void> {
    let p = gpa(mono, name.as_ptr());
    (!p.is_null()).then_some(p)
}

/// Turns every counter section on. Call before `mono_jit_init_version`: Mono registers its counters during
/// initialisation and skips sections that are not enabled.
pub unsafe fn enable(mono: win::Hmodule, gpa: unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void) -> bool {
    match get(mono, gpa, c"mono_counters_enable") {
        Some(f) => {
            let f: unsafe extern "C" fn(i32) = std::mem::transmute(f);
            f(ALL_SECTIONS);
            true
        }
        None => false,
    }
}

/// "section | name = value" for every registered counter. Must run on a thread attached to Mono.
pub unsafe fn read(mono: win::Hmodule, gpa: unsafe extern "system" fn(win::Hmodule, *const c_char) -> *mut c_void) -> Vec<String> {
    let api = match (|| -> Option<Api> {
        Some(Api {
            foreach: std::mem::transmute(get(mono, gpa, c"mono_counters_foreach")?),
            name: std::mem::transmute(get(mono, gpa, c"mono_counter_get_name")?),
            kind: std::mem::transmute(get(mono, gpa, c"mono_counter_get_type")?),
            section: std::mem::transmute(get(mono, gpa, c"mono_counter_get_section")?),
            sample: std::mem::transmute(get(mono, gpa, c"mono_counters_sample")?),
        })
    })() {
        Some(a) => a,
        None => return vec!["counters unavailable: Mono lacks the counters API".into()],
    };
    let mut out: (Vec<String>, &Api) = (Vec::new(), &api);
    (api.foreach)(collect, &mut out as *mut _ as *mut c_void);
    out.0
}

unsafe extern "C" fn collect(counter: Counter, user: *mut c_void) -> i32 {
    let (lines, api) = &mut *(user as *mut (Vec<String>, &Api));
    let name = (api.name)(counter);
    let name = if name.is_null() { "?".into() } else { CStr::from_ptr(name).to_string_lossy() };
    let mut buf = [0u8; 256];
    let n = (api.sample)(counter, buf.as_mut_ptr().cast(), buf.len() as i32);
    let value = if n <= 0 { "(no value)".to_string() } else { format_value((api.kind)(counter) & TYPE_MASK, &buf[..n as usize]) };
    lines.push(format!("{} | {} = {}", section_name((api.section)(counter)), name, value));
    1 // keep going
}

fn format_value(kind: i32, b: &[u8]) -> String {
    let i64_at = || b.get(..8).map(|s| i64::from_le_bytes(s.try_into().unwrap()));
    match kind {
        INT => b.get(..4).map(|s| i32::from_le_bytes(s.try_into().unwrap()).to_string()),
        UINT => b.get(..4).map(|s| u32::from_le_bytes(s.try_into().unwrap()).to_string()),
        WORD | LONG => i64_at().map(|v| v.to_string()),
        ULONG => b.get(..8).map(|s| u64::from_le_bytes(s.try_into().unwrap()).to_string()),
        DOUBLE => b.get(..8).map(|s| format!("{:.3}", f64::from_le_bytes(s.try_into().unwrap()))),
        // Microseconds.
        TIME_INTERVAL => i64_at().map(|v| format!("{:.3} s", v as f64 / 1e6)),
        STRING => Some(String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or(b)).into_owned()),
        _ => None,
    }
    .unwrap_or_else(|| format!("(type {})", kind))
}

fn section_name(section: i32) -> &'static str {
    match section >> 8 {
        1 => "jit",
        2 => "gc",
        4 => "metadata",
        8 => "generics",
        16 => "security",
        32 => "runtime",
        64 => "system",
        128 => "profiler",
        _ => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_each_type() {
        assert_eq!(format_value(INT, &(-5i32).to_le_bytes()), "-5");
        assert_eq!(format_value(UINT, &7u32.to_le_bytes()), "7");
        assert_eq!(format_value(LONG, &12345678901i64.to_le_bytes()), "12345678901");
        assert_eq!(format_value(DOUBLE, &1.5f64.to_le_bytes()), "1.500");
        assert_eq!(format_value(TIME_INTERVAL, &2_500_000i64.to_le_bytes()), "2.500 s");
        assert_eq!(format_value(STRING, b"abc\0junk"), "abc");
        assert_eq!(format_value(LONG, &[1, 2]), "(type 3)");
        assert_eq!(section_name(1 << 8), "jit");
        assert_eq!(section_name(1 << 10), "metadata");
    }
}
