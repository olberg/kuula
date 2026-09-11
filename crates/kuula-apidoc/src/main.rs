//! `kuula-apidoc --check [path]` fails when the generated sections of
//! `docs/api.md` are stale; `kuula-apidoc --write [path]` regenerates
//! them in place. The path defaults to this repository's `docs/api.md`.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, path) = match args.as_slice() {
        [mode] => (mode.as_str(), kuula_apidoc::default_path()),
        [mode, path] => (mode.as_str(), path.into()),
        _ => return usage(),
    };
    let doc = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("cannot read {}: {e}", path.display());
            return ExitCode::from(2);
        }
    };
    match mode {
        "--check" => match kuula_apidoc::check(&doc) {
            Ok(()) => {
                println!("{} is up to date", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                ExitCode::from(1)
            }
        },
        "--write" => match kuula_apidoc::render(&doc) {
            Ok(fresh) => {
                if fresh == doc {
                    println!("{} is up to date", path.display());
                    return ExitCode::SUCCESS;
                }
                if let Err(e) = std::fs::write(&path, fresh) {
                    eprintln!("cannot write {}: {e}", path.display());
                    return ExitCode::from(2);
                }
                println!("wrote {}", path.display());
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{}: {e}", path.display());
                ExitCode::from(1)
            }
        },
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("usage: kuula-apidoc --check|--write [docs/api.md]");
    ExitCode::from(2)
}
