use std::path::Path;

fn main() {
    println!("cargo:rerun-if-changed=web/dist");
    assert!(
        Path::new("web/dist/index.html").is_file()
            && Path::new("web/dist/static/js/ghostty-vt.wasm").is_file(),
        "Browser assets are missing; run `make build-web` before building the crate."
    );
}
