//! 실제 offline Cargo 해석과 실행 marker로 revision 설정 권위와 기존 caller 정책을 분리한다.
#![cfg(unix)]
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicUsize, Ordering};

static NEXT: AtomicUsize = AtomicUsize::new(0);
const CONFIG: &str =
    "[source.crates-io]\nreplace-with='local'\n[source.local]\ndirectory='vendor-a'\n";
struct Fixture {
    root: PathBuf,
    project: PathBuf,
    caller: PathBuf,
    home: PathBuf,
    audit: PathBuf,
    bin: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rustograph-context-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let project = root.join("project");
        let caller = root.join("caller");
        let home = root.join("home");
        let audit = root.join("audit");
        let bin = root.join("bin");
        for path in [&project, &caller, &home, &audit, &bin] {
            std::fs::create_dir(path).unwrap();
        }
        let fixture = Self {
            root,
            project,
            caller,
            home,
            audit,
            bin,
        };
        fixture.write(&fixture.project, "Cargo.toml", "[package]\nname='authority_probe'\nversion='0.1.0'\nedition='2021'\n[dependencies]\nrustograph-authority-probe-dep='1.0.0'\n");
        fixture.write(
            &fixture.project,
            "src/lib.rs",
            "pub fn selected() { revision_base::selected(); }\n",
        );
        fixture.write(&fixture.project, ".gitignore", "/target/\n");
        fixture.write(&fixture.project, ".cargo/config.toml", CONFIG);
        for (base, folder, library) in [
            (&fixture.project, "vendor-a", "revision_base"),
            (&fixture.project, "vendor-b", "revision_head"),
            (&fixture.caller, "vendor-a", "caller_dep"),
        ] {
            let vendor = base.join(folder).join("authority_dep");
            fixture.write(&vendor, "Cargo.toml", &format!("[package]\nname='rustograph-authority-probe-dep'\nversion='1.0.0'\nedition='2021'\n[lib]\nname='{library}'\n"));
            fixture.write(&vendor, "src/lib.rs", "pub fn selected() {}\n");
            fixture.write(
                &vendor,
                ".cargo-checksum.json",
                "{\"files\":{},\"package\":null}\n",
            );
        }
        fixture.write(&fixture.caller, ".cargo/config.toml", CONFIG);
        fixture.write(&fixture.bin, "cargo", "#!/bin/sh\nfile=\"$RUSTOGRAPH_CONTEXT_AUDIT/metadata-$$.json\"\nprintf '%s\\n' \"$file\" >>\"$RUSTOGRAPH_CONTEXT_AUDIT/order\"\nprintf '%s\\n' \"$CARGO_HOME\" >>\"$RUSTOGRAPH_CONTEXT_AUDIT/homes\"\n\"$RUSTOGRAPH_REAL_CARGO\" \"$@\" >\"$file\"\nstatus=$?\ncat \"$file\"\nexit \"$status\"\n");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            fixture.bin.join("cargo"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fixture.generate_lock(&fixture.project);
        fixture.git(&["init", "-q"]);
        fixture.commit();
        fixture
    }
    fn write(&self, base: &Path, path: &str, text: &str) {
        let path = base.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    fn git(&self, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "core.hooksPath=",
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "-C",
            ])
            .arg(&self.project)
            .args(args)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    }
    fn commit(&self) {
        self.git(&["add", "."]);
        self.git(&["commit", "-qm", "public configuration fixture"]);
    }
    fn generate_lock(&self, directory: &Path) {
        let output = Command::new(env!("CARGO"))
            .args(["generate-lockfile", "--offline", "--manifest-path"])
            .arg(directory.join("Cargo.toml"))
            .current_dir(directory)
            .env("CARGO_HOME", &self.home)
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTC_WRAPPER", "")
            .env("RUSTC_WORKSPACE_WRAPPER", "")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    fn command(&self, directory: &Path) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_rustograph"));
        let search = std::env::var_os("PATH").unwrap();
        let paths = std::iter::once(self.bin.clone()).chain(std::env::split_paths(&search));
        command
            .current_dir(&self.caller)
            .env("PATH", std::env::join_paths(paths).unwrap())
            .env("CARGO_HOME", &self.home)
            .env("CARGO_NET_OFFLINE", "true")
            .env("RUSTOGRAPH_CONTEXT_AUDIT", &self.audit)
            .env("RUSTOGRAPH_REAL_CARGO", env!("CARGO"))
            .args(["change-impact", "--dir"])
            .arg(directory)
            .args(["--base", "HEAD", "--working-tree"]);
        command
    }
    fn observed_targets(&self) -> Vec<String> {
        let Ok(order) = std::fs::read_to_string(self.audit.join("order")) else {
            return Vec::new();
        };
        order
            .lines()
            .map(|path| {
                let document: Value =
                    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
                document["packages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|package| package["name"] == "rustograph-authority-probe-dep")
                    .unwrap()["targets"][0]["name"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }
    fn succeeded(output: &Output) -> Value {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn captured_revisions_use_their_own_offline_vendor_configuration() {
    let fixture = Fixture::new();
    fixture.write(
        &fixture.project,
        ".cargo/config.toml",
        &CONFIG.replace("vendor-a", "vendor-b"),
    );
    fixture.write(
        &fixture.project,
        "src/lib.rs",
        "pub fn selected() { revision_head::selected(); }\n",
    );
    let report = Fixture::succeeded(&fixture.command(&fixture.project).output().unwrap());
    assert_eq!(
        fixture.observed_targets(),
        ["revision_base", "revision_head"]
    );
    assert_eq!(report["before"]["provenance"]["freshness"], "captured");
    assert_eq!(report["after"]["provenance"]["freshness"], "captured");
    assert!(report["after"]["limitations"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value
            .as_str()
            .unwrap()
            .contains("offline neutral working directory")));
}

#[test]
fn ordinary_graph_loading_keeps_the_existing_caller_cargo_policy() {
    let fixture = Fixture::new();
    // 새 capture seam이 아니라 기존 public graph/load 경로를 사용한다.
    let search = std::env::var_os("PATH").unwrap();
    let paths = std::iter::once(fixture.bin.clone()).chain(std::env::split_paths(&search));
    let mut command = Command::new(env!("CARGO_BIN_EXE_rustograph"));
    let output = command
        .current_dir(&fixture.caller)
        .env("PATH", std::env::join_paths(paths).unwrap())
        .env("CARGO_HOME", &fixture.home)
        .env("CARGO_NET_OFFLINE", "true")
        .env("RUSTOGRAPH_CONTEXT_AUDIT", &fixture.audit)
        .env("RUSTOGRAPH_REAL_CARGO", env!("CARGO"))
        .args(["graph", "--dir"])
        .arg(&fixture.project)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fixture.observed_targets(), ["caller_dep"]);
}

#[test]
fn path_only_patches_and_path_overrides_are_not_silently_dropped() {
    for prefix in [
        "paths=['vendor-b/authority_dep']\n",
        "[patch.crates-io.rustograph-authority-probe-dep]\npath='vendor-b/authority_dep'\n",
    ] {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.project,
            ".cargo/config.toml",
            &format!("{prefix}{CONFIG}"),
        );
        fixture.generate_lock(&fixture.project);
        fixture.commit();
        Fixture::succeeded(&fixture.command(&fixture.project).output().unwrap());
        assert_eq!(
            fixture.observed_targets(),
            ["revision_head", "revision_head"]
        );
    }
}

#[test]
fn relative_member_config_paths_use_the_parent_of_its_cargo_directory() {
    let fixture = Fixture::new();
    let member = fixture.project.join("member");
    std::fs::create_dir(&member).unwrap();
    std::fs::rename(
        fixture.project.join("Cargo.toml"),
        member.join("Cargo.toml"),
    )
    .unwrap();
    std::fs::rename(fixture.project.join("src"), member.join("src")).unwrap();
    fixture.write(
        &fixture.project,
        "Cargo.toml",
        "[workspace]\nmembers=['member']\nresolver='2'\n",
    );
    fixture.write(
        &member,
        ".cargo/config.toml",
        "[source.local]\ndirectory='../vendor-b'\n",
    );
    fixture.generate_lock(&member);
    fixture.commit();
    Fixture::succeeded(&fixture.command(&member).output().unwrap());
    assert_eq!(
        fixture.observed_targets(),
        ["revision_head", "revision_head"]
    );
}

#[test]
fn unsupported_settings_are_refused_before_cargo_or_project_programs_run() {
    for extra in [
        "[env]\nRUSTC_WRAPPER={value='./marker.sh',force=true}\n",
        "include=['other.toml']\n",
        "[build]\nrustc-wrapper='./marker.sh'\n",
        "[registry]\nglobal-credential-providers=['./marker.sh']\n",
    ] {
        let fixture = Fixture::new();
        fixture.write(
            &fixture.project,
            "marker.sh",
            "#!/bin/sh\ntouch \"$RUSTOGRAPH_EXECUTION_MARKER\"\nexit 99\n",
        );
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            fixture.project.join("marker.sh"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        fixture.write(&fixture.project, ".cargo/config.toml", extra);
        fixture.commit();
        let output = fixture
            .command(&fixture.project)
            .env("RUSTOGRAPH_EXECUTION_MARKER", fixture.root.join("executed"))
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(String::from_utf8_lossy(&output.stderr).contains("captured Cargo configuration"));
        assert!(fixture.observed_targets().is_empty());
        assert!(!fixture.root.join("executed").exists());
    }
}

#[test]
fn configuration_symlinks_and_oversized_files_are_refused_without_opening_them() {
    for linked in [true, false] {
        let fixture = Fixture::new();
        if linked {
            let config = fixture.project.join(".cargo/config.toml");
            std::fs::remove_file(&config).unwrap();
            std::os::unix::fs::symlink(fixture.caller.join(".cargo/config.toml"), config).unwrap();
        } else {
            fixture.write(
                &fixture.project,
                ".cargo/config.toml",
                &format!("#{}\n", "x".repeat(1024 * 1024)),
            );
        }
        fixture.commit();
        let output = fixture.command(&fixture.project).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(fixture.observed_targets().is_empty());
    }
}

#[test]
fn a_neutral_scratch_ancestor_configuration_is_refused_without_using_it() {
    let fixture = Fixture::new();
    let scratch = fixture.root.join("scratch");
    std::fs::create_dir(&scratch).unwrap();
    fixture.write(
        &scratch,
        ".cargo/config.toml",
        "[env]\nRUSTOGRAPH_UNTRUSTED='marker'\n",
    );
    let output = fixture
        .command(&fixture.project)
        .env("TMPDIR", &scratch)
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(String::from_utf8_lossy(&output.stderr).contains("neutral ancestor"));
    assert!(fixture.observed_targets().is_empty());
}

#[test]
fn caller_relative_cargo_home_keeps_its_identity_after_neutral_cwd_selection() {
    let fixture = Fixture::new();
    let relative_home = fixture.caller.join("selected-home");
    std::fs::create_dir(&relative_home).unwrap();
    let output = fixture
        .command(&fixture.project)
        .env("CARGO_HOME", "selected-home")
        .output()
        .unwrap();
    Fixture::succeeded(&output);
    assert_eq!(
        fixture.observed_targets(),
        ["revision_base", "revision_base"]
    );
    assert!(relative_home.is_dir());
    // 선택한 상대 home이 실제 Cargo 자식에서도 caller 기준으로 유지됐는지 확인한다.
    let selected = relative_home.to_str().unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.audit.join("homes"))
            .unwrap()
            .lines()
            .collect::<Vec<_>>(),
        [selected, selected]
    );
}

#[test]
fn relative_rustup_home_is_explicitly_refused_before_neutral_toolchain_resolution() {
    let fixture = Fixture::new();
    let output = fixture
        .command(&fixture.project)
        .env("RUSTUP_HOME", "selected-rustup")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(
        error.contains("relative Rustup/home selection needs an absolute path"),
        "{error}"
    );
    assert!(fixture.observed_targets().is_empty());
    assert!(!fixture.caller.join("selected-rustup").exists());
}

#[test]
fn invalid_toml_path_escapes_and_source_replacements_are_errors_without_echoing_values() {
    for text in [
        "paths=[]\npaths=[]\n",
        "paths=['../DO_NOT_LOG_MARKER']\n",
        "[source.crates-io]\nreplace-with='missing'\n",
        "[source.a]\nreplace-with='b'\n[source.b]\nreplace-with='a'\n",
        "\"env\".TOKEN='DO_NOT_LOG_MARKER'\n",
    ] {
        let fixture = Fixture::new();
        fixture.write(&fixture.project, ".cargo/config.toml", text);
        fixture.commit();
        let output = fixture.command(&fixture.project).output().unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("captured Cargo configuration"), "{error}");
        assert!(!error.contains("DO_NOT_LOG_MARKER"), "{error}");
        assert!(fixture.observed_targets().is_empty());
    }
}
