//! The driver itself (Windows only; main.rs is the entry point).
#[path = "metadata.rs"]
mod metadata;
#[path = "platform.rs"]
mod platform;
#[path = "worker.rs"]
mod worker;
use std::{
    collections::{BTreeMap, BTreeSet},
    env, fs,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
type Result<T> = std::result::Result<T, String>;
const FORMAT: &str = "FASTBOOT-AOT-1";
#[derive(Clone)]
struct Item {
    path: PathBuf,
    hash: String,
    assembly: Option<metadata::Assembly>,
}
struct Inventory {
    items: Vec<Item>,
    providers: BTreeMap<String, Vec<usize>>,
}
#[derive(Default)]
struct Args {
    command: String,
    ksp: PathBuf,
    cache: PathBuf,
    linker: PathBuf,
    assemblies: Vec<PathBuf>,
    refs: PathBuf,
    input: PathBuf,
    timeout: u64,
}
fn plain(p: &Path) -> Result<String> {
    let s = p
        .to_str()
        .ok_or("non-Unicode Windows path is unsupported by this Mono host")?;
    Ok(s.strip_prefix(r"\\?\").unwrap_or(s).to_string())
}
fn canonical(p: &Path) -> Result<PathBuf> {
    fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))
}
fn destination(p: &Path) -> Result<PathBuf> {
    if p.exists() {
        return canonical(p);
    }
    let absolute = if p.is_absolute() {
        p.to_path_buf()
    } else {
        env::current_dir().map_err(|e| e.to_string())?.join(p)
    };
    let parent = absolute.parent().ok_or("cache has no parent")?;
    let name = absolute.file_name().ok_or("invalid cache path")?;
    Ok(destination(parent)?.join(name))
}
fn text(p: &Path) -> Result<String> {
    fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))
}
fn write(p: &Path, s: &str) -> Result<()> {
    let mut f = fs::File::create(p).map_err(|e| e.to_string())?;
    f.write_all(s.as_bytes()).map_err(|e| e.to_string())?;
    f.sync_all().map_err(|e| e.to_string())
}
fn hex(s: &str) -> String {
    s.as_bytes().iter().map(|b| format!("{b:02x}")).collect()
}
fn unhex(s: &str) -> Result<String> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err("invalid hex field".into());
    }
    let b: std::result::Result<Vec<_>, _> = (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
        .collect();
    String::from_utf8(b.map_err(|_| "invalid hex")?).map_err(|_| "invalid UTF8 field".into())
}
fn beneath(path: &Path, root: &Path) -> bool {
    path.to_string_lossy()
        .to_lowercase()
        .starts_with(&(root.to_string_lossy().trim_end_matches('\\').to_lowercase() + "\\"))
        || path
            .to_string_lossy()
            .eq_ignore_ascii_case(&root.to_string_lossy())
}
fn gather(dir: &Path, all: bool, out: &mut Vec<PathBuf>) -> Result<()> {
    for entry in fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        let m = fs::symlink_metadata(&p).map_err(|e| e.to_string())?;
        use std::os::windows::fs::MetadataExt;
        if m.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "reparse point requires explicit resolver support: {}",
                p.display()
            ));
        }
        if m.is_dir() {
            gather(&p, all, out)?;
        } else if m.is_file() {
            let ext = p
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();
            if all
                || ext == "dll"
                || ext == "exe"
                || ext == "config"
                || ext == "mdb"
                || ext == "pdb"
            {
                out.push(p);
            }
        }
    }
    Ok(())
}
fn inventory(ksp: &Path, extra: &[PathBuf]) -> Result<Inventory> {
    let mut paths = Vec::new();
    gather(&ksp.join("KSP_x64_Data/Managed"), false, &mut paths)?;
    gather(&ksp.join("GameData"), false, &mut paths)?;
    gather(&ksp.join("MonoBleedingEdge/etc"), true, &mut paths)?;
    paths.push(ksp.join("MonoBleedingEdge/EmbedRuntime/mono-2.0-bdwgc.dll"));
    for name in ["UnityPlayer.dll", "KSP_x64.exe"] {
        let p = ksp.join(name);
        if p.is_file() {
            paths.push(p);
        }
    }
    paths.extend_from_slice(extra);
    for p in extra {
        let config = PathBuf::from(format!("{}.config", plain(p)?));
        if config.is_file() {
            paths.push(config);
        }
    }
    for p in &mut paths {
        *p = canonical(p)?;
    }
    paths.sort();
    paths.dedup();
    let mut items = Vec::new();
    let mut providers: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for path in paths {
        let hash = platform::hash_file(&path)?;
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let assembly = if ext.eq_ignore_ascii_case("dll") || ext.eq_ignore_ascii_case("exe") {
            metadata::read(&path).map_err(|e| format!("{}: {e}", path.display()))?
        } else {
            None
        };
        if let Some(a) = &assembly {
            providers
                .entry(a.name.to_ascii_lowercase())
                .or_default()
                .push(items.len());
        }
        items.push(Item {
            path,
            hash,
            assembly,
        });
    }
    // Identical duplicate assemblies are interchangeable; differing bytes remain ambiguous.
    for indices in providers.values_mut() {
        indices.sort_by_key(|i| !beneath(&items[*i].path, &ksp.join("KSP_x64_Data/Managed")));
        let mut seen = BTreeSet::new();
        indices.retain(|i| seen.insert(items[*i].hash.clone()));
    }
    Ok(Inventory { items, providers })
}
fn eligible(inv: &Inventory, i: usize, seen: &mut BTreeSet<usize>) -> Result<()> {
    if !seen.insert(i) {
        return Ok(());
    }
    let a = inv.items[i]
        .assembly
        .as_ref()
        .ok_or("not a managed assembly")?;
    let own = inv
        .providers
        .get(&a.name.to_ascii_lowercase())
        .ok_or("missing target provider")?;
    if own.len() != 1 {
        return Err(format!("ambiguous assembly identity {}", a.name));
    }
    for name in &a.refs {
        let refs = inv
            .providers
            .get(&name.to_ascii_lowercase())
            .ok_or_else(|| format!("{} requires missing {name}", a.name))?;
        if refs.len() != 1 {
            return Err(format!("{} requires ambiguous {name}", a.name));
        }
        eligible(inv, refs[0], seen)?;
    }
    Ok(())
}
fn identity(a: &Args, inv: &Inventory) -> Result<(String, String)> {
    let mut s = format!(
        "{FORMAT}\noptions\t{} {}\ncpu\t{}\nksp\t{}\ndriver\t{}\nlinker\t{}\n",
        worker::OPTIMIZER,
        worker::OPTIONS,
        platform::cpu_identity(),
        hex(&plain(&a.ksp)?),
        platform::hash_file(&env::current_exe().map_err(|e| e.to_string())?)?,
        platform::hash_file(&a.linker)?
    );
    // This is deliberately a conservative whole-install epoch, including dependencies and configuration.
    for item in &inv.items {
        s += &format!("input\t{}\t{}\n", hex(&plain(&item.path)?), item.hash);
    }
    for path in &a.assemblies {
        s += &format!("target\t{}\n", hex(&plain(path)?));
    }
    Ok((platform::hash_bytes(s.as_bytes())?, s))
}
fn parse() -> Result<Args> {
    let mut it = env::args().skip(1);
    let mut a = Args {
        command: it.next().unwrap_or_else(|| "help".into()),
        timeout: 180,
        ..Default::default()
    };
    while let Some(key) = it.next() {
        let value = it
            .next()
            .ok_or_else(|| format!("missing value for {key}"))?;
        match key.as_str() {
            "--ksp" => a.ksp = PathBuf::from(value),
            "--cache" => a.cache = PathBuf::from(value),
            "--linker" => a.linker = PathBuf::from(value),
            "--assembly" => a.assemblies.push(PathBuf::from(value)),
            "--refs" => a.refs = PathBuf::from(value),
            "--input" => a.input = PathBuf::from(value),
            "--timeout" => a.timeout = value.parse().map_err(|_| "invalid timeout")?,
            _ => return Err(format!("unknown option {key}")),
        }
    }
    Ok(a)
}
fn run_process(mut cmd: Command, log: &Path, timeout: u64) -> Result<()> {
    use std::os::windows::process::CommandExt;
    let job = platform::Job::new()?;
    let file = fs::File::create(log).map_err(|e| e.to_string())?;
    let err = file.try_clone().map_err(|e| e.to_string())?;
    cmd.stdin(Stdio::null())
        .stdout(file)
        .stderr(err)
        .creation_flags(0x08000000 | 0x4000); // hidden, below-normal
    let mut child = cmd.spawn().map_err(|e| format!("spawn failed: {e}"))?;
    if let Err(e) = job.assign(&child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(e);
    }
    let start = Instant::now();
    let mut last_guard = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(format!("worker exited {status}"))
                }
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e.to_string());
            }
            _ => {}
        }
        let failure = if start.elapsed() > Duration::from_secs(timeout) {
            Some("worker timeout".to_string())
        } else if last_guard.elapsed() > Duration::from_secs(1) {
            last_guard = Instant::now();
            platform::idle_guard().err()
        } else {
            None
        };
        if let Some(e) = failure {
            let _ = child.kill();
            let _ = child.wait();
            return Err(e);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}
fn safe_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name
            .chars()
            .any(|c| c < ' ' || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'))
    {
        return Err("unsafe cache filename".into());
    }
    Ok(())
}
fn verify(a: &Args) -> Result<PathBuf> {
    let current = text(&a.cache.join("current.txt"))?;
    let current = current.trim();
    safe_name(current)?;
    let dir = a.cache.join(current);
    let manifest = text(&dir.join("manifest.tsv"))?;
    let mut args = Args {
        ksp: a.ksp.clone(),
        linker: a.linker.clone(),
        ..Default::default()
    };
    let mut key = None;
    let mut outputs = Vec::new();
    if manifest.lines().next() != Some(FORMAT) {
        return Err("unsupported manifest format".into());
    }
    for line in manifest.lines().skip(1) {
        let cols: Vec<_> = line.split('\t').collect();
        match cols.as_slice() {
            ["key", v] => {
                if key.replace(*v).is_some() {
                    return Err("duplicate manifest key".into());
                }
            }
            ["target", p] => args.assemblies.push(canonical(Path::new(&unhex(p)?))?),
            ["output", name, hash] => {
                let name = unhex(name)?;
                safe_name(&name)?;
                outputs.push((name, hash.to_string()));
            }
            _ => {}
        }
    }
    if outputs.is_empty() {
        return Err("cache contains no successful images".into());
    }
    let expected: BTreeSet<_> = outputs.iter().map(|(n, _)| n.clone()).collect();
    let mut found = BTreeSet::new();
    for entry in fs::read_dir(dir.join("images")).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        found.insert(
            entry
                .file_name()
                .to_str()
                .ok_or("invalid image filename")?
                .to_string(),
        );
    }
    if found != expected {
        return Err("unlisted or missing files in native image directory".into());
    }
    let inv = inventory(&args.ksp, &args.assemblies)?;
    if Some(identity(&args, &inv)?.0.as_str()) != key {
        return Err("cache inputs, CPU, options or tools changed".into());
    }
    for (name, hash) in outputs {
        if platform::hash_file(&dir.join("images").join(name))? != hash {
            return Err("cached native image changed".into());
        }
    }
    Ok(dir.join("images"))
}
fn build(a: &Args) -> Result<i32> {
    platform::low_priority()?;
    platform::idle_guard()?;
    if a.assemblies.is_empty() {
        return Err(
            "build requires explicit --assembly selections; use plan and the JIT profile first"
                .into(),
        );
    }
    let inv = inventory(&a.ksp, &a.assemblies)?;
    let (key, identity) = identity(a, &inv)?;
    if let Ok(images) = verify(a) {
        let manifest = text(&images.parent().unwrap().join("manifest.tsv"))?;
        if manifest.lines().any(|l| l == format!("key\t{key}")) {
            println!("cache-hit\t{}", plain(&images)?);
            return Ok(0);
        }
    }
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| e.to_string())?
        .as_nanos();
    let unique = format!("{}-{}-{stamp}", &key[..24], std::process::id());
    let staging = a.cache.join(format!(".building-{unique}"));
    fs::create_dir(&staging).map_err(|e| e.to_string())?;
    // Windows releases this exclusive handle on crash. A leftover filename is harmless.
    use std::os::windows::fs::OpenOptionsExt;
    let _lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .share_mode(0)
        .open(a.cache.join("build.lock"))
        .map_err(|e| format!("another builder holds the cache lock: {e}"))?;
    let refs = staging.join("refs");
    let images = staging.join("images");
    fs::create_dir(&refs).map_err(|e| e.to_string())?;
    fs::create_dir(&images).map_err(|e| e.to_string())?;
    for indices in inv.providers.values().filter(|v| v.len() == 1) {
        let item = &inv.items[indices[0]];
        let name = format!("{}.dll", item.assembly.as_ref().unwrap().name);
        safe_name(&name)?;
        let dest = refs.join(name);
        fs::copy(&item.path, &dest).map_err(|e| e.to_string())?;
        if platform::hash_file(&dest)? != item.hash {
            return Err("source changed while taking dependency snapshot".into());
        }
    }
    let mut manifest = format!("{FORMAT}\nkey\t{key}\n");
    for target in &a.assemblies {
        manifest += &format!("target\t{}\n", hex(&plain(target)?));
    }
    let mut outputs = 0;
    let mut failures = 0;
    let mut names = BTreeSet::new();
    for (n, target) in a.assemblies.iter().enumerate() {
        let task = staging.join(format!("assembly-{n:04}"));
        fs::create_dir(&task).map_err(|e| e.to_string())?;
        let result = (|| -> Result<(String, String)> {
            platform::idle_guard()?;
            let i = inv
                .items
                .iter()
                .position(|i| &i.path == target)
                .ok_or("target missing from inventory")?;
            eligible(&inv, i, &mut BTreeSet::new())?;
            let item = &inv.items[i];
            let filename = target
                .file_name()
                .and_then(|n| n.to_str())
                .ok_or("invalid target filename")?;
            safe_name(filename)?;
            let image_name = format!("{filename}.dll");
            if !names.insert(image_name.to_lowercase()) {
                return Err("two targets have the same AOT cache filename".into());
            }
            let input = task.join(filename);
            fs::copy(target, &input).map_err(|e| e.to_string())?;
            if platform::hash_file(&input)? != item.hash {
                return Err("target changed while taking snapshot".into());
            }
            // Preserve per-assembly dllmap/config alongside the snapshotted target.
            let config = PathBuf::from(format!("{}.config", plain(target)?));
            if config.is_file() {
                fs::copy(&config, task.join(format!("{filename}.config")))
                    .map_err(|e| e.to_string())?;
            }
            let mut cmd = Command::new(env::current_exe().map_err(|e| e.to_string())?);
            cmd.arg("worker")
                .arg("--ksp")
                .arg(&a.ksp)
                .arg("--refs")
                .arg(&refs)
                .arg("--input")
                .arg(&input)
                .current_dir(&task);
            run_process(cmd, &task.join("compile.log"), a.timeout)?;
            if fs::metadata(task.join("image.s"))
                .map_err(|e| e.to_string())?
                .len()
                == 0
            {
                return Err("compiler produced an empty file".into());
            }
            // Failed or partial linker files never enter the runtime's search directory.
            let output = task.join(&image_name);
            let mut cmd = Command::new(&a.linker);
            cmd.arg(task.join("image.s"))
                .arg(&output)
                .current_dir(&task);
            run_process(cmd, &task.join("link.log"), a.timeout)?;
            let bytes = fs::read(&output).map_err(|e| e.to_string())?;
            metadata::validate_aot(&bytes)?;
            let hash = platform::hash_file(&output)?;
            fs::rename(&output, images.join(&image_name)).map_err(|e| e.to_string())?;
            Ok((image_name, hash))
        })();
        match result {
            Ok((name, hash)) => {
                outputs += 1;
                manifest += &format!("output\t{}\t{hash}\n", hex(&name));
                println!("compiled\t{}", plain(target)?);
            }
            Err(e) => {
                failures += 1;
                manifest += &format!("failed\t{}\t{}\n", hex(&plain(target)?), hex(&e));
                eprintln!("skipped\t{}\t{e}", plain(target)?);
            }
        }
    }
    write(&staging.join("identity.tsv"), &identity)?;
    write(&staging.join("manifest.tsv"), &manifest)?;
    if outputs == 0 {
        return Err(format!(
            "no usable images; diagnostics at {}",
            plain(&staging)?
        ));
    }
    // Re-enumeration catches new/removed dependencies as well as in-place edits, not just size/mtime changes.
    if self::identity(a, &inventory(&a.ksp, &a.assemblies)?)?.0 != key {
        return Err("installation changed during compilation; generation not published".into());
    }
    let final_dir = a.cache.join(&unique);
    fs::rename(&staging, &final_dir).map_err(|e| e.to_string())?;
    let pointer = a.cache.join(format!("current-{}.tmp", std::process::id()));
    write(&pointer, &format!("{unique}\n"))?;
    platform::replace(&pointer, &a.cache.join("current.txt"))?;
    println!(
        "published\t{}\t{outputs} images, {failures} failures",
        plain(&final_dir.join("images"))?
    );
    Ok(if failures == 0 { 0 } else { 3 })
}
fn run() -> Result<i32> {
    let mut a = parse()?;
    if a.command == "help" || a.command == "--help" {
        println!("aot-compile plan --ksp <root>\naot-compile build --ksp <root> --cache <outside GameData> --linker <aotlink.exe> --assembly <path>... [--timeout 180]\naot-compile verify --ksp <root> --cache <dir> --linker <aotlink.exe>\nBuild is an offline, low-priority process; it defers while KSP runs. No Unity entry point is executed.");
        return Ok(0);
    }
    a.ksp = canonical(&a.ksp)?;
    if a.command == "worker" {
        return worker::run(&a.ksp, &a.refs, &a.input);
    }
    for p in &mut a.assemblies {
        *p = canonical(p)?;
    }
    a.assemblies.sort();
    a.assemblies.dedup();
    if a.command == "plan" {
        let inv = inventory(&a.ksp, &a.assemblies)?;
        println!("methods\tname\tmvid_bytes\tstatus\tpath");
        let mut indices: Vec<_> = inv
            .items
            .iter()
            .enumerate()
            .filter(|(_, i)| i.assembly.is_some())
            .collect();
        indices.sort_by_key(|(_, i)| std::cmp::Reverse(i.assembly.as_ref().unwrap().methods));
        for (i, item) in indices {
            let asm = item.assembly.as_ref().unwrap();
            let status = eligible(&inv, i, &mut BTreeSet::new())
                .err()
                .unwrap_or_else(|| "eligible".into());
            println!(
                "{}\t{}\t{}\t{}\t{}",
                asm.methods,
                asm.name,
                asm.mvid,
                status,
                plain(&item.path)?
            );
        }
        return Ok(0);
    }
    if !matches!(a.command.as_str(), "build" | "verify") {
        return Err("unknown command".into());
    }
    if a.cache.as_os_str().is_empty() || a.linker.as_os_str().is_empty() {
        return Err("--cache and --linker are required".into());
    }
    a.linker = canonical(&a.linker)?;
    a.cache = destination(&a.cache)?;
    if beneath(&a.cache, &a.ksp.join("GameData"))
        || beneath(&a.cache, &a.ksp.join("KSP_x64_Data"))
        || beneath(&a.cache, &a.ksp.join("MonoBleedingEdge"))
    {
        return Err("cache must be outside GameData and runtime folders".into());
    }
    if a.command == "build" {
        fs::create_dir_all(&a.cache).map_err(|e| e.to_string())?;
    }
    a.cache = canonical(&a.cache)?;
    if a.command == "verify" {
        println!("{}", plain(&verify(&a)?)?);
        Ok(0)
    } else {
        build(&a)
    }
}
pub fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("aot-compile: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn manifest_fields_roundtrip() {
        for value in ["C:\\folder with spaces\\test.dll", "München\t\n", ""] {
            assert_eq!(unhex(&hex(value)).unwrap(), value);
        }
        assert!(unhex("f").is_err());
        assert!(unhex("zz").is_err());
    }
    #[test]
    fn refuses_path_escape() {
        for n in ["../a", "..", "C:\\a", "a/b"] {
            assert!(safe_name(n).is_err());
        }
        assert!(safe_name("Assembly-CSharp.dll.dll").is_ok());
    }
    #[test]
    fn containment_observes_path_boundary() {
        assert!(beneath(
            Path::new("C:\\KSP\\GameData\\Cache"),
            Path::new("c:\\ksp\\gamedata")
        ));
        assert!(!beneath(
            Path::new("C:\\KSP\\GameDataOther"),
            Path::new("C:\\KSP\\GameData")
        ));
    }
}
