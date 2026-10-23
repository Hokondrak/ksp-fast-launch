//! FastBoot's optional early-start loader, shipped as `version.dll` next to `KSP_x64.exe`.
//!
//! KSP reads every GameData config once before it loads any plugin, so a GameData-only mod cannot speed up
//! that first pass. Unity's player (`UnityPlayer.dll`) imports `VERSION.dll` and resolves the Mono embedding API
//! through kernel32's `GetProcAddress`, which gives a dependency-free way in:
//!
//! 1. Windows loads this DLL in place of the system `version.dll`; every export forwards to the real one
//!    ([`proxy`]).
//! 2. On load, the player's import slot for `GetProcAddress` is pointed at [`early::get_proc_address`]
//!    ([`iat`]). Nothing else in the process is touched.
//! 3. When the player asks for `mono_jit_init_version`, it gets a wrapper. After the real call returns, the
//!    wrapper opens `GameData/FastBoot/Plugins/FastBoot.dll` and calls `FastBoot.Early.Bootstrap.Start`, then
//!    restores the import slot.
//! 4. The managed side waits for Assembly-CSharp and installs the config parser before KSP's first scan.
//!
//! Every failure path leaves the game as it would be without this file: the import slot is restored or never
//! changed, and FastBoot's normal GameData addon still starts the parser later. Kill switches: delete this
//! file, create `GameData/FastBoot/PluginData/disable-early`, or set `FASTBOOT_EARLY_DISABLE=1`.
//!
//! What happened is written to `GameData/FastBoot/PluginData/early.log`.

#[cfg(windows)]
mod early;
#[cfg(windows)]
mod iat;
#[cfg(windows)]
mod log;
#[cfg(windows)]
mod proxy;
#[cfg(windows)]
mod win;

#[cfg(windows)]
#[no_mangle]
pub unsafe extern "system" fn DllMain(module: win::Hmodule, reason: u32, _reserved: *mut core::ffi::c_void) -> i32 {
    const DLL_PROCESS_ATTACH: u32 = 1;
    if reason == DLL_PROCESS_ATTACH {
        win::DisableThreadLibraryCalls(module);
        // A panic must never cross into the loader.
        let _ = std::panic::catch_unwind(|| early::attach());
    }
    1
}
