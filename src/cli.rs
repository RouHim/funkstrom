use std::env;
use std::path::PathBuf;

pub fn get_config_path() -> PathBuf {
    let args: Vec<String> = env::args().collect();
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
