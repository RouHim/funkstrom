fn main() {
    let version = std::env::var("FUNKSTROM_VERSION")
        .unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string());
    let out_dir = std::env::var("OUT_DIR").unwrap();
    std::fs::write(
        std::path::Path::new(&out_dir).join("version.txt"),
        format!("funkstrom/{}", version),
    )
    .unwrap();
}
