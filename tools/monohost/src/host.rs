//! Windows implementation: load Unity's Mono by full path and drive its embedding API.

use std::ffi::{c_char, c_int, c_void, CString};
use std::path::{Path, PathBuf};

type Fn1 = unsafe extern "C" fn(*const c_char);
type Fn2 = unsafe extern "C" fn(*const c_char, *const c_char);
type InitFn = unsafe extern "C" fn(*const c_char, *const c_char) -> *mut c_void;
type OpenFn = unsafe extern "C" fn(*mut c_void, *const c_char) -> *mut c_void;
type ExecFn = unsafe extern "C" fn(*mut c_void, *mut c_void, c_int, *mut *mut c_char) -> c_int;
type ExitCodeFn = unsafe extern "C" fn() -> c_int;

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const c_char) -> *mut c_void;
    fn SetDllDirectoryW(path: *const u16) -> i32;
}

fn wide(p: &Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    p.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

fn cstr(s: &str) -> CString {
    CString::new(s).expect("argument contains a NUL character")
}

struct Mono(*mut c_void);

impl Mono {
    unsafe fn get<T: Copy>(&self, name: &str) -> Result<T, String> {
        let c = cstr(name);
        let p = GetProcAddress(self.0, c.as_ptr());
        if p.is_null() {
            return Err(format!("Mono export {} not found", name));
        }
        Ok(std::mem::transmute_copy(&p))
    }
}

struct Args {
    ksp: PathBuf,
    preload: Vec<PathBuf>,
    program: PathBuf,
    rest: Vec<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut ksp = None;
    let mut preload = Vec::new();
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--ksp" => ksp = it.next().map(PathBuf::from),
            "--preload" => preload.push(it.next().map(PathBuf::from).ok_or("missing DLL after --preload")?),
            "--" => {
                let program = it.next().ok_or("missing program after --")?;
                let ksp = ksp.ok_or("missing --ksp <KSP folder>")?;
                return Ok(Args { ksp, preload, program: PathBuf::from(program), rest: it.collect() });
            }
            _ => return Err(format!("unexpected argument {}", a)),
        }
    }
    Err("usage: monohost --ksp <KSP folder> [--preload <dll>]... -- <program.exe> [args...]".into())
}

pub fn main() -> i32 {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("monohost: {}", e);
            return 2;
        }
    };
    match unsafe { run(&args) } {
        Ok(code) => code,
        Err(e) => {
            eprintln!("monohost: {}", e);
            1
        }
    }
}

unsafe fn run(args: &Args) -> Result<i32, String> {
    // Loaded before Mono, the way Windows loads UnityPlayer.dll's imports (FastBoot's version.dll among them).
    for dll in &args.preload {
        if LoadLibraryW(wide(dll).as_ptr()).is_null() {
            return Err(format!("could not preload {}", dll.display()));
        }
    }
    let runtime_dir = args.ksp.join("MonoBleedingEdge").join("EmbedRuntime");
    let etc_dir = args.ksp.join("MonoBleedingEdge").join("etc");
    let managed = args.ksp.join("KSP_x64_Data").join("Managed");
    let dll = runtime_dir.join("mono-2.0-bdwgc.dll");
    if !dll.is_file() {
        return Err(format!("{} not found", dll.display()));
    }
    // The runtime's own dependencies sit next to it.
    SetDllDirectoryW(wide(&runtime_dir).as_ptr());
    let handle = LoadLibraryW(wide(&dll).as_ptr());
    if handle.is_null() {
        return Err(format!("could not load {}", dll.display()));
    }
    let mono = Mono(handle);
    let set_assemblies_path: Fn1 = mono.get("mono_set_assemblies_path")?;
    let set_dirs: Fn2 = mono.get("mono_set_dirs")?;
    let config_parse: Fn1 = mono.get("mono_config_parse")?;
    let jit_init: InitFn = mono.get("mono_jit_init_version")?;
    let open: OpenFn = mono.get("mono_domain_assembly_open")?;
    let exec: ExecFn = mono.get("mono_jit_exec")?;
    let exit_code: ExitCodeFn = mono.get("mono_environment_exitcode_get")?;

    // Same layout Unity's player uses: class libraries from Managed, config from MonoBleedingEdge/etc.
    let managed_c = cstr(&managed.display().to_string());
    let etc_c = cstr(&etc_dir.display().to_string());
    set_dirs(managed_c.as_ptr(), etc_c.as_ptr());
    set_assemblies_path(managed_c.as_ptr());
    config_parse(std::ptr::null());
    let domain = jit_init(cstr("monohost").as_ptr(), cstr("v4.0.30319").as_ptr());
    if domain.is_null() {
        return Err("mono_jit_init_version failed".into());
    }
    let program = std::fs::canonicalize(&args.program).map_err(|e| format!("{}: {}", args.program.display(), e))?;
    let program_s = program.display().to_string();
    let program_s = program_s.strip_prefix(r"\\?\").unwrap_or(&program_s).to_string();
    let assembly = open(domain, cstr(&program_s).as_ptr());
    if assembly.is_null() {
        return Err(format!("Mono could not open {}", program_s));
    }
    let mut owned: Vec<CString> = std::iter::once(program_s).chain(args.rest.iter().cloned()).map(|s| cstr(&s)).collect();
    let mut argv: Vec<*mut c_char> = owned.iter_mut().map(|s| s.as_ptr() as *mut c_char).collect();
    let ret = exec(domain, assembly, argv.len() as c_int, argv.as_mut_ptr());
    // mono_jit_exec returns Main's value for int Main; Environment.ExitCode otherwise.
    let code = if ret != 0 { ret } else { exit_code() };
    Ok(code)
}
