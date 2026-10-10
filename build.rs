use std::{env, fs, io::ErrorKind, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=.env");

    let manifest_dir = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").expect("Cargo must set CARGO_MANIFEST_DIR"),
    );
    let env_path = manifest_dir.join(".env");
    let contents = match fs::read_to_string(&env_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == ErrorKind::NotFound => String::new(),
        Err(error) => panic!("could not read .env for embedding: {error}"),
    };

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("Cargo must set OUT_DIR"));
    let embedded_env = format!("pub const EMBEDDED_ENV: &str = {contents:?};\n");
    fs::write(out_dir.join("embedded_env.rs"), embedded_env)
        .expect("could not write embedded .env source");
}
