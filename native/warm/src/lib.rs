//! Cold-start warm-up.
//!
//! On a cold start (after a reboot, or once other programs have pushed KSP's files out of the OS file cache) a
//! modded KSP spends most of its boot waiting for the disk: on the reference install, 139 s cold against 61 s
//! warm. KSP and Unity read files one at a time, which leaves an SSD mostly idle. This crate reads the same files
//! ahead of the game on several threads, in the order the game will want them, so the game's own reads are served
//! from memory. It changes nothing on disk and nothing in the game; at worst it reads files the game then does not
//! need.
//!
//! - [`plan`] decides what to read and in what order: the files KSP opened during the previous boot ([`readlist`],
//!   recorded with [`record`]), followed by everything KSP's loaders would read, in loading-phase order.
//! - [`run`] reads them within a memory budget from [`system`].

pub mod plan;
pub mod readlist;
pub mod record;
pub mod run;
pub mod settings;
pub mod system;

/// Lower-case, '/'-separated form of a path, for comparing paths the way Windows does.
pub(crate) fn path_key(p: &str) -> String {
    p.replace('\\', "/").to_lowercase()
}
