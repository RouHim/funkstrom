use clap::{Arg, Command};
use std::path::PathBuf;

pub fn build_cli() -> Command {
    Command::new(env!("CARGO_PKG_NAME"))
        .version(env!("CARGO_PKG_VERSION"))
        .about(env!("CARGO_PKG_DESCRIPTION"))
        .arg(
            Arg::new("config")
                .short('c')
                .long("config")
                .value_name("FILE")
                .help("Sets a custom config file")
                .default_value("./data/config.toml"),
        )
}

pub fn get_config_path() -> PathBuf {
    let matches = build_cli().get_matches();
    PathBuf::from(matches.get_one::<String>("config").unwrap())
}
