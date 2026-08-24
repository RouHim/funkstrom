use std::env;
use std::path::PathBuf;

pub fn get_config_path() -> PathBuf {
    let args: Vec<String> = env::args().collect();

    // Handle --help / -h
    if args.iter().any(|a| a == "-h" || a == "--help") {
        eprintln!("Usage: funkstrom [OPTIONS]");
        eprintln!();
        eprintln!("Options:");
        eprintln!("  -c, --config <PATH>  Path to config file [default: ./config.toml]");
        eprintln!("  -h, --help           Print help");
        std::process::exit(0);
    }

    let mut config_path = PathBuf::from("./config.toml");
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-c" | "--config" if i + 1 < args.len() => {
                config_path = PathBuf::from(&args[i + 1]);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    config_path
}

#[cfg(test)]
mod tests {
    use super::*;

    // Characterization notes for `get_config_path`.
    //
    // The function reads `std::env::args()` directly instead of taking an
    // argument slice, so argument-driven branches cannot be exercised
    // in-process: Rust offers no portable way to rewrite the running test
    // binary's argv. The branches below are therefore pinned by inspection,
    // exactly as the current implementation behaves:
    //
    // * `-h` / `--help` anywhere in argv: prints usage to stderr, then
    //   `std::process::exit(0)` — not testable here (it would terminate the
    //   whole test process).
    // * `-c <PATH>` / `--config <PATH>`: sets the returned path to `<PATH>`
    //   (only when a following argument exists).
    // * `--config=<PATH>` as a single token: NOT supported — it matches the
    //   catch-all `_` arm and is silently ignored, keeping the default path.
    // * A trailing `-c` / `--config` with no following argument: the
    //   `i + 1 < args.len()` guard fails, so it is ignored and the default
    //   path is kept.
    // * Any other token (unknown flags, stray positionals): silently ignored.

    #[test]
    fn given_invocation_without_config_or_help_args_when_get_config_path_then_returns_default_path()
    {
        // Precondition check must run before `get_config_path`: a `-h`/`--help`
        // passthrough arg would make the production code exit the process.
        let passthrough: Vec<String> = env::args().skip(1).collect();
        let has_config_flag = passthrough
            .iter()
            .enumerate()
            .any(|(i, arg)| matches!(arg.as_str(), "-c" | "--config") && i + 1 < passthrough.len());
        let has_help_flag = passthrough
            .iter()
            .any(|arg| matches!(arg.as_str(), "-h" | "--help"));
        assert!(
            !has_config_flag && !has_help_flag,
            "precondition violated: test harness passed CLI flags {passthrough:?}; \
             this test assumes a plain `cargo test` invocation"
        );

        assert_eq!(get_config_path(), PathBuf::from("./config.toml"));
    }

    #[test]
    fn given_repeated_calls_when_get_config_path_then_result_is_stable() {
        assert_eq!(get_config_path(), get_config_path());
    }
}
