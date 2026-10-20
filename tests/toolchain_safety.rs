//! 분석 대상 체크아웃의 cargo·rustup 설정이 rustograph가 실행할 프로그램을 고르지 못하는지 — 실제
//! 바이너리를 체크아웃 안에서 띄워, 설정이 가리키는 흔적 스크립트가 실행되지 않고 결과가 체크아웃 밖에서
//! 돌린 것과 같은지 본다. 흔적 스크립트가 셸 스크립트라 unix에서만 돈다.

#[cfg(unix)]
mod checkout_configuration {
    use serde_json::Value;
    use std::ffi::{OsStr, OsString};
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    /// 체크아웃 안 설정에 흔적 스크립트를 심은 분석 대상 한 벌과, 그 밖의 흔적 디렉터리.
    struct Checkout {
        /// 둘을 담는 임시 디렉터리 — 끝나면 지운다.
        root: PathBuf,
        /// 흔적 스크립트와 마커가 놓이는 체크아웃 밖 디렉터리 — 밖에서 돌리는 실행의 작업 디렉터리다.
        kit: PathBuf,
        /// 분석 대상 워크스페이스(체크아웃).
        workspace: PathBuf,
    }

    impl Checkout {
        fn new(tag: &str) -> Checkout {
            let root =
                std::env::temp_dir().join(format!("rg-toolchain-{tag}-{}", std::process::id()));
            if root.exists() {
                std::fs::remove_dir_all(&root).unwrap();
            }
            let kit = root.join("kit");
            let workspace = root.join("checkout");
            std::fs::create_dir_all(&kit).unwrap();
            std::fs::create_dir_all(workspace.join("src")).unwrap();
            std::fs::write(
                workspace.join("Cargo.toml"),
                "[package]\nname = \"checkout\"\nversion = \"0.1.0\"\nedition = \"2021\"\n\n[workspace]\n",
            )
            .unwrap();
            std::fs::write(
                workspace.join("src/lib.rs"),
                "pub fn entry() {\n    helper()\n}\nfn helper() {}\n",
            )
            .unwrap();
            Checkout {
                root,
                kit,
                workspace,
            }
        }

        /// 체크아웃 안 파일을 쓴다(중간 디렉터리 포함).
        fn write(&self, relative: &str, text: &str) {
            let path = self.workspace.join(relative);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        /// 실행되면 마커 `name`을 남기고 `then`(셸 문장)을 잇는 스크립트를 `dir`에 `file`로 만든다.
        fn script_in(&self, dir: &Path, file: &str, name: &str, then: &str) -> PathBuf {
            use std::os::unix::fs::PermissionsExt;
            std::fs::create_dir_all(dir).unwrap();
            let path = dir.join(file);
            let marker = self.kit.join(format!("MARKER-{name}"));
            let body = format!("#!/bin/sh\ntouch '{}'\n{then}\n", marker.display());
            std::fs::write(&path, body).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        /// 마커만 남기고 실패하는 흔적 스크립트 — 실행됐다는 사실만 본다.
        fn script(&self, name: &str) -> PathBuf {
            self.script_in(&self.kit, &format!("{name}.sh"), name, "exit 1")
        }

        /// 지금까지 실행된 흔적 스크립트의 마커 이름(정렬).
        fn fired(&self) -> Vec<String> {
            let mut fired: Vec<String> = std::fs::read_dir(&self.kit)
                .unwrap()
                .filter_map(|e| e.ok()?.file_name().into_string().ok())
                .filter_map(|n| n.strip_prefix("MARKER-").map(str::to_string))
                .collect();
            fired.sort();
            fired
        }

        /// 체크아웃 안에서 띄운다(기본값 `--dir .`과 같은 상황) — `--dir`은 밖 실행과 같은 절대 경로다.
        fn run_inside(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Output {
            self.run_from(&self.workspace, args, env)
        }

        /// 체크아웃 밖(흔적 디렉터리)에서 같은 워크스페이스를 `--dir`로 분석한다 — 비교 기준이다.
        fn run_outside(&self, args: &[&str], env: &[(&str, &OsStr)]) -> Output {
            self.run_from(&self.kit, args, env)
        }

        /// 사용자가 RUSTUP_TOOLCHAIN·RUSTC를 주지 않은 셸처럼 띄운다 — rustup 프록시로 돈 `cargo test`는
        /// 자식에게 RUSTUP_TOOLCHAIN을 물려주므로 지운다. `env`가 그 위에 값을 준다.
        fn run_from(&self, cwd: &Path, args: &[&str], env: &[(&str, &OsStr)]) -> Output {
            let mut command = Command::new(env!("CARGO_BIN_EXE_rustograph"));
            command
                .args(args)
                .arg("--dir")
                .arg(&self.workspace)
                .current_dir(cwd)
                .env_remove("RUSTUP_TOOLCHAIN")
                .env_remove("RUSTC");
            for (key, value) in env {
                command.env(key, value);
            }
            command.output().unwrap()
        }
    }

    impl Drop for Checkout {
        fn drop(&mut self) {
            // 테스트가 실패해 풀리는 중에도 불리므로 패닉하지 않고 알린다.
            if let Err(e) = std::fs::remove_dir_all(&self.root) {
                eprintln!("could not remove {}: {e}", self.root.display());
            }
        }
    }

    fn assert_success(output: &Output, args: &[&str]) {
        assert_eq!(
            output.status.code(),
            Some(0),
            "{args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn json(output: &Output) -> Value {
        serde_json::from_slice(&output.stdout).expect("output must be JSON")
    }

    /// 문서의 `limitations`에서 `label`로 시작하는 항목을 빼고 그 수를 센다 — 목록이 비면 키를 뺀다
    /// (값 없는 선택 필드는 키가 없다는 출력 규칙).
    fn without_label(mut doc: Value, label: &str) -> (Value, usize) {
        let Some(list) = doc.get_mut("limitations").and_then(Value::as_array_mut) else {
            return (doc, 0);
        };
        let before = list.len();
        list.retain(|l| !l.as_str().is_some_and(|s| s.starts_with(label)));
        let removed = before - list.len();
        if list.is_empty() {
            doc.as_object_mut().unwrap().remove("limitations");
        }
        (doc, removed)
    }

    /// PATH의 절대 경로 항목에서 rustup이 있는 디렉터리를 찾는다 — 없으면 rustup 경로 테스트를 건너뛴다.
    fn rustup_dir() -> Option<PathBuf> {
        let path = std::env::var_os("PATH")?;
        std::env::split_paths(&path)
            .filter(|dir| dir.is_absolute())
            .find(|dir| dir.join("rustup").is_file())
    }

    /// rustup 프록시가 PATH 맨 앞에 오게 한다 — 로컬 PATH가 다른 cargo를 먼저 찾아도 프록시 경로를 시험한다.
    fn proxies_first(rustup_dir: &Path) -> OsString {
        let mut entries = vec![rustup_dir.to_path_buf()];
        entries.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        std::env::join_paths(entries).unwrap()
    }

    /// rustup의 기본 툴체인 이름과 그 rustc 경로 — 체크아웃 밖에서 묻는다(테스트 준비 단계).
    fn default_toolchain(rustup_dir: &Path, cwd: &Path) -> (String, PathBuf) {
        let rustup = rustup_dir.join("rustup");
        let shown = Command::new(&rustup)
            .arg("default")
            .current_dir(cwd)
            .env_remove("RUSTUP_TOOLCHAIN")
            .output()
            .unwrap();
        assert!(shown.status.success(), "rustup default failed");
        let name = String::from_utf8(shown.stdout).unwrap();
        let name = name.split_whitespace().next().unwrap().to_string();
        let which = Command::new(&rustup)
            .args(["which", "rustc"])
            .current_dir(cwd)
            .env("RUSTUP_TOOLCHAIN", &name)
            .output()
            .unwrap();
        assert!(which.status.success(), "rustup which rustc failed");
        let rustc = PathBuf::from(String::from_utf8(which.stdout).unwrap().trim());
        (name, rustc)
    }

    const HARVESTS: [&[&str]; 3] = [
        &["graph", "--level", "symbol"],
        &["deps", "--format", "json"],
        &[
            "graph",
            "--level",
            "symbol",
            "--target",
            "wasm32-unknown-unknown",
        ],
    ];

    #[test]
    fn checkout_cargo_config_does_not_pick_the_programs_that_run() {
        let checkout = Checkout::new("config");
        let wrapper = checkout.script("rustc-wrapper");
        let workspace_wrapper = checkout.script("rustc-workspace-wrapper");
        let rustc = checkout.script("build-rustc");
        checkout.write(
            ".cargo/config.toml",
            &format!(
                "[build]\nrustc-wrapper = {:?}\nrustc-workspace-wrapper = {:?}\nrustc = {:?}\n",
                wrapper.display().to_string(),
                workspace_wrapper.display().to_string(),
                rustc.display().to_string()
            ),
        );
        for args in HARVESTS {
            let inside = checkout.run_inside(args, &[]);
            assert_success(&inside, args);
            assert_eq!(
                checkout.fired(),
                Vec::<String>::new(),
                "{args:?} ran a program that the checkout's .cargo/config.toml names"
            );
            let outside = checkout.run_outside(args, &[]);
            assert_success(&outside, args);
            assert_eq!(
                json(&inside),
                json(&outside),
                "{args:?}: inside and outside runs differ"
            );
        }
    }

    #[test]
    fn checkout_toolchain_file_does_not_pick_the_toolchain() {
        let Some(rustup_dir) = rustup_dir() else {
            eprintln!("skipping: rustup is not on an absolute PATH entry");
            return;
        };
        let path = proxies_first(&rustup_dir);
        let checkout = Checkout::new("toolchain");
        let fake = checkout.kit.join("fake-toolchain");
        for tool in ["cargo", "rustc"] {
            checkout.script_in(
                &fake.join("bin"),
                tool,
                &format!("toolchain-{tool}"),
                "exit 1",
            );
        }
        checkout.write(
            "rust-toolchain.toml",
            &format!("[toolchain]\npath = {:?}\n", fake.display().to_string()),
        );
        let env = [("PATH", path.as_os_str())];
        for args in HARVESTS {
            let inside = checkout.run_inside(args, &env);
            assert_success(&inside, args);
            assert_eq!(
                checkout.fired(),
                Vec::<String>::new(),
                "{args:?} ran the toolchain that the checkout's rust-toolchain.toml names"
            );
            let (inside, ignored) = without_label(json(&inside), "toolchain-file-ignored:");
            assert_eq!(ignored, 1, "{args:?}: toolchain-file-ignored limitation");
            let outside = checkout.run_outside(args, &env);
            assert_success(&outside, args);
            assert_eq!(
                inside,
                json(&outside),
                "{args:?}: inside and outside runs differ"
            );
        }
    }

    #[test]
    fn toolchain_choices_from_the_environment_still_apply() {
        let Some(rustup_dir) = rustup_dir() else {
            eprintln!("skipping: rustup is not on an absolute PATH entry");
            return;
        };
        let path = proxies_first(&rustup_dir);
        let checkout = Checkout::new("user-choice");
        let fake = checkout.kit.join("fake-toolchain");
        checkout.script_in(&fake.join("bin"), "cargo", "toolchain-cargo", "exit 1");
        checkout.write(
            "rust-toolchain.toml",
            &format!("[toolchain]\npath = {:?}\n", fake.display().to_string()),
        );
        let (name, real_rustc) = default_toolchain(&rustup_dir, &checkout.kit);
        let args = &["graph", "--level", "symbol"];
        // 사용자가 고른 툴체인은 존중한다 — 체크아웃의 툴체인 파일을 무시한 것이 아니라 사용자가 덮은 것이라
        // limitation도 없다.
        let chosen = checkout.run_inside(
            args,
            &[
                ("PATH", path.as_os_str()),
                ("RUSTUP_TOOLCHAIN", OsStr::new(&name)),
            ],
        );
        assert_success(&chosen, args);
        assert_eq!(checkout.fired(), Vec::<String>::new());
        assert_eq!(without_label(json(&chosen), "toolchain-file-ignored:").1, 0);
        // 사용자가 준 RUSTC는 그대로 실행된다.
        let user_rustc = checkout.script_in(
            &checkout.kit,
            "user-rustc.sh",
            "user-rustc",
            &format!("exec '{}' \"$@\"", real_rustc.display()),
        );
        let given = checkout.run_inside(
            args,
            &[
                ("PATH", path.as_os_str()),
                ("RUSTC", user_rustc.as_os_str()),
            ],
        );
        assert_success(&given, args);
        assert_eq!(checkout.fired(), vec!["user-rustc".to_string()]);
        assert_eq!(without_label(json(&given), "toolchain-file-ignored:").1, 1);
    }

    #[test]
    fn relative_path_entries_do_not_resolve_inside_the_checkout() {
        let checkout = Checkout::new("relative-path");
        for tool in ["cargo", "rustc", "rustup"] {
            checkout.script_in(
                &checkout.workspace,
                tool,
                &format!("checkout-{tool}"),
                "exit 1",
            );
            checkout.script_in(
                &checkout.workspace.join("bin"),
                tool,
                &format!("checkout-bin-{tool}"),
                "exit 1",
            );
        }
        let mut entries = vec![PathBuf::new(), PathBuf::from("."), PathBuf::from("bin")];
        entries.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        let path = std::env::join_paths(entries).unwrap();
        for args in HARVESTS {
            let inside = checkout.run_inside(args, &[("PATH", path.as_os_str())]);
            assert_success(&inside, args);
            assert_eq!(
                checkout.fired(),
                Vec::<String>::new(),
                "{args:?} ran a program found through a relative PATH entry"
            );
        }
    }
}
