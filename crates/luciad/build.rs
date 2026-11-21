//! 관리 화면(결정 0061)의 정적 빌드(`web/admin/dist`)를 바이너리에 넣는다. 화면과 데몬이 한 파일로 함께 깔리므로
//! 둘의 판이 어긋나지 않는다. 빌드가 없으면(Node 없이 `cargo test`만 돌릴 때) 안내 한 장을 대신 넣는다:
//! `luciad check`가 그것을 알아보고, `deploy/install.sh`는 늘 화면을 먼저 빌드한다.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, base, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push((rel.to_string_lossy().replace('\\', "/"), path));
        }
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let dist = manifest.join("../../web/admin/dist");
    println!("cargo:rerun-if-changed={}", dist.display());
    println!(
        "cargo:rerun-if-changed={}",
        dist.join("index.html").display()
    );
    println!("cargo:rerun-if-changed={}", dist.join("assets").display());

    let mut files = Vec::new();
    if dist.join("index.html").is_file() {
        walk(&dist, &dist, &mut files);
        files.sort();
    }
    let mut code = String::from("pub static FILES: &[(&str, &[u8])] = &[\n");
    for (name, path) in &files {
        let abs = path.canonicalize().expect("dist file");
        writeln!(
            code,
            "    ({name:?}, include_bytes!({:?})),",
            abs.display().to_string()
        )
        .unwrap();
    }
    code.push_str("];\n");
    writeln!(code, "pub const BUILT: bool = {};", !files.is_empty()).unwrap();
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("out dir")).join("admin_ui.rs");
    std::fs::write(out, code).expect("write admin_ui.rs");
}
