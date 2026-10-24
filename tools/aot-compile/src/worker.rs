//! This process invokes Mono's compiler, never an assembly entry point.
use super::{plain, platform};
use std::{
    ffi::{c_char, c_int, c_void, CString},
    path::Path,
};
pub const OPTIONS: &str = "asmonly,bind-to-runtime-version,threads=1,no-direct-calls";
pub const OPTIMIZER: &str = "--optimize=-inline";
fn c(s: &str) -> Result<CString, String> {
    CString::new(s).map_err(|_| "NUL in argument".into())
}
unsafe fn symbol<T: Copy>(h: *mut c_void, name: &str) -> Result<T, String> {
    let p = platform::GetProcAddress(h, c(name)?.as_ptr());
    if p.is_null() {
        return Err(format!("missing Mono export {name}"));
    }
    Ok(std::mem::transmute_copy(&p))
}
pub fn run(ksp: &Path, refs: &Path, input: &Path) -> Result<i32, String> {
    platform::low_priority()?;
    let runtime = ksp.join("MonoBleedingEdge/EmbedRuntime");
    let managed = ksp.join("KSP_x64_Data/Managed");
    let etc = ksp.join("MonoBleedingEdge/etc");
    // No inherited MONO_ENV_OPTIONS, MONO_PATH, debugger or profiler can alter this compilation.
    for (key, _) in std::env::vars_os() {
        if key
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("MONO_")
        {
            std::env::remove_var(key);
        }
    }
    unsafe {
        platform::SetDllDirectoryW(platform::wide(&runtime).as_ptr());
        let h =
            platform::LoadLibraryW(platform::wide(&runtime.join("mono-2.0-bdwgc.dll")).as_ptr());
        if h.is_null() {
            return Err(format!(
                "cannot load installed Mono: {}",
                std::io::Error::last_os_error()
            ));
        }
        let dirs: unsafe extern "C" fn(*const c_char, *const c_char) = symbol(h, "mono_set_dirs")?;
        let paths: unsafe extern "C" fn(*const c_char) = symbol(h, "mono_set_assemblies_path")?;
        let config: unsafe extern "C" fn(*const c_char) = symbol(h, "mono_config_parse")?;
        let main: unsafe extern "C" fn(c_int, *mut *mut c_char) -> c_int = symbol(h, "mono_main")?;
        dirs(c(&plain(&managed)?)?.as_ptr(), c(&plain(&etc)?)?.as_ptr());
        paths(c(&plain(refs)?)?.as_ptr());
        config(std::ptr::null());
        // Relative output avoids commas/quotes in arbitrary user paths entering Mono's option parser.
        let args = [
            c("aot-compile-worker")?,
            c(OPTIMIZER)?,
            c(&format!("--aot={OPTIONS},outfile=image.s"))?,
            c(&plain(input)?)?,
        ];
        let mut argv: Vec<_> = args.iter().map(|s| s.as_ptr() as *mut c_char).collect();
        Ok(main(argv.len() as c_int, argv.as_mut_ptr()))
    }
}
