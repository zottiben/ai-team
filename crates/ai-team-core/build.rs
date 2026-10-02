use std::{env, fmt::Write, fs, path::PathBuf};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("assets/toolbox");
    println!("cargo:rerun-if-changed=assets/toolbox");
    let manifest = fs::read_to_string(root.join("FILES")).unwrap();
    let mut source = String::from("const ASSETS: &[(&str, &[u8], u32, &str)] = &[\n");
    for line in manifest.lines() {
        let fields: Vec<_> = line.splitn(3, ' ').collect();
        assert_eq!(fields.len(), 3, "invalid toolbox manifest line");
        let [mode, digest, path] = [fields[0], fields[1], fields[2]];
        assert!(matches!(mode, "644" | "755"));
        assert!(!path.starts_with('/') && !path.split('/').any(|p| p == ".."));
        // Debug on a str emits the quoted/escaped Rust string literal we need.
        let file = root.join(path);
        let file = file.to_str().expect("UTF-8 asset path");
        writeln!(
            source,
            "({path:?}, include_bytes!({file:?}), 0o{mode}, {digest:?}),"
        )
        .unwrap();
    }
    source.push_str("];");
    fs::write(
        PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("toolbox_assets.rs"),
        source,
    )
    .unwrap();
}
