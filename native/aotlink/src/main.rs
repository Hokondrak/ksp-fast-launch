//! aotlink: `aotlink <input.s> <output.dll>` turns Mono's `--aot=asmonly` output into an AOT image DLL.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() != 2 {
        eprintln!("usage: aotlink <input.s> <output.dll>");
        std::process::exit(2);
    }
    let source = match std::fs::read_to_string(&args[0]) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("aotlink: {}: {}", args[0], e);
            std::process::exit(1);
        }
    };
    let name = std::path::Path::new(&args[1]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    match fastboot_aotlink::link(&source, &name) {
        Ok(bytes) => {
            if let Err(e) = std::fs::write(&args[1], bytes) {
                eprintln!("aotlink: {}: {}", args[1], e);
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("aotlink: {}", e);
            std::process::exit(1);
        }
    }
}
