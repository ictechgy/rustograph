//! compiler가 받는 모듈 파일을 잘못된 파일명/타깃 기준 디렉터리로 잃지 않는지 검증한다.

use rustograph::{graph::EdgeKind, source};
use std::path::PathBuf;
use std::process::Command;

struct Fixture(PathBuf);

impl Fixture {
    fn new(name: &str, manifest: &str, files: &[(&str, &str)]) -> Self {
        let root =
            std::env::temp_dir().join(format!("rustograph-modules-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("Cargo.toml"), manifest).unwrap();
        for (name, text) in files {
            let path = root.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(root)
    }

    fn compile(&self, file: &str, kind: &str, name: &str) {
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let output = Command::new(rustc)
            .args([
                "--edition",
                "2021",
                "--crate-type",
                kind,
                "--crate-name",
                name,
                file,
                "-o",
            ])
            .arg(self.0.join(format!("checked-{name}-{kind}")))
            .current_dir(&self.0)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn graph(&self) -> rustograph::graph::Document {
        source::load(
            &self.0,
            &source::Options {
                symbol_level: true,
                ..Default::default()
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn raw_module_names_use_unescaped_filesystem_names_and_literal_path_overrides() {
    let f = Fixture::new("raw", "[package]\nname='raw_module'\nversion='0.1.0'\nedition='2021'\n[workspace]\n", &[
        ("src/lib.rs", "pub mod r#type;\npub mod r#loop { pub mod nested; }\n#[path=\"r#literal.rs\"] pub mod explicit;\npub fn entry() { r#type::keyword(); r#loop::nested::leaf(); explicit::literal() }\n"),
        ("src/type.rs", "pub fn keyword() {}\n"),
        ("src/loop/nested.rs", "pub fn leaf() {}\n"),
        ("src/r#literal.rs", "pub fn literal() {}\n"),
    ]);
    f.compile("src/lib.rs", "lib", "raw_module");
    let d = f.graph();
    for target in [
        "raw_module::r#type::keyword",
        "raw_module::r#loop::nested::leaf",
        "raw_module::explicit::literal",
    ] {
        assert!(
            d.vertices.iter().any(|v| v.id == target),
            "compiler accepted declared source missing: {target}"
        );
        assert!(d.edges.iter().any(|e| e.from == "raw_module::entry"
            && e.to == target
            && e.kind == EdgeKind::Call
            && !e.tentative));
    }
    assert!(!d.limitations.iter().any(|l| l.contains("orphans")));
}

#[test]
fn merged_lib_bin_uses_actual_target_file_for_external_inline_and_path_modules() {
    let f = Fixture::new("merged", "[package]\nname='merged_target'\nversion='0.1.0'\nedition='2021'\nautobins=false\n[[bin]]\nname='merged_target'\npath='src/bin/main.rs'\n[workspace]\n", &[
        ("src/lib.rs", "pub fn library() {}\n"),
        ("src/bin/main.rs", "mod outer;\nmod inline { pub mod child; }\n#[path=\"special.rs\"] mod special;\nfn main() { outer::call(); inline::child::call(); special::call() }\n"),
        ("src/bin/outer.rs", "pub fn call() {}\n"),
        ("src/bin/inline/child.rs", "pub fn call() {}\n"),
        ("src/bin/special.rs", "pub fn call() {}\n"),
    ]);
    f.compile("src/lib.rs", "lib", "merged_target");
    f.compile("src/bin/main.rs", "bin", "merged_target");
    let d = f.graph();
    assert_eq!(
        d.vertices
            .iter()
            .filter(|v| v.kind == rustograph::graph::Kind::Crate)
            .count(),
        1,
        "target namespace model stays merged"
    );
    for target in [
        "merged_target::outer::call",
        "merged_target::inline::child::call",
        "merged_target::special::call",
    ] {
        assert!(
            d.vertices.iter().any(|v| v.id == target),
            "compiler accepted bin module missing: {target}"
        );
        assert!(d.edges.iter().any(|e| e.from == "merged_target::main"
            && e.to == target
            && e.kind == EdgeKind::Call
            && !e.tentative));
    }
    assert!(!d.limitations.iter().any(|l| l.contains("orphans")));
}
