//! Git 변경을 revision별 소스 수확과 연결한다. 두 그래프의 간선을 합치지 않는다.
use crate::analysis::{self, ImpactResult};
use crate::graph::Kind;
use crate::{source, traversal::sha256::Sha256};
use serde::Serialize;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

/// 파일 단위 보수적 seed의 출력/작업 상한. 잘리면 결과가 이를 명시한다.
pub const MAX_SEEDS: usize = 256;
static NEXT: AtomicUsize = AtomicUsize::new(0);

/// MCP가 반복 수확한 AST를 세션에 누적하지 않게 짧은 자식 프로세스에서 같은 CLI를 실행한다.
pub(crate) fn isolated_json(request: &Request<'_>) -> Result<String, String> {
    let executable =
        std::env::current_exe().map_err(|e| format!("locate rustograph executable: {e}"))?;
    let mut command = Command::new(executable);
    command
        .args(["change-impact", "--dir"])
        .arg(request.dir)
        .args([
            "--base",
            request.base,
            "--depth",
            &request.depth.to_string(),
            "--max",
            &request.max.to_string(),
        ]);
    if let Some(head) = request.head {
        command.args(["--head", head]);
    }
    if request.working_tree {
        command.arg("--working-tree");
    }
    let output = command
        .output()
        .map_err(|e| format!("start isolated change analysis: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    String::from_utf8(output.stdout).map_err(|_| "change analysis returned invalid UTF-8".into())
}

/// CLI와 MCP가 같은 변경 수확·순회를 사용한다.
pub struct Request<'a> {
    pub dir: &'a Path,
    pub base: &'a str,
    pub head: Option<&'a str>,
    pub working_tree: bool,
    /// 0은 깊이 제한 없음. 출력 상한은 계속 적용된다.
    pub depth: usize,
    /// revision 하나에 싣는 전체 impacted 행의 상한.
    pub max: usize,
}

/// 서로 독립적으로 수확·탐색한 변경 분석 결과.
#[derive(Serialize)]
pub struct Report {
    pub mode: &'static str,
    pub state: &'static str,
    pub changes: Vec<Change>,
    pub before: Snapshot,
    pub after: Snapshot,
    pub limitations: Vec<String>,
    pub preparation_ms: u128,
    pub interpretation: &'static str,
}

/// rename은 이전/이후 이름을 함께 보존한다.
#[derive(Serialize)]
pub struct Change {
    pub status: String,
    pub old_path: Option<String>,
    pub new_path: Option<String>,
}

/// 한 revision의 seed, 호출자, 실제 간선 경로. 다른 revision과 연결하지 않는다.
#[derive(Serialize)]
pub struct Snapshot {
    pub provenance: Provenance,
    pub seeds: Vec<Seed>,
    pub impacts: Vec<ImpactResult>,
    /// 수확이 보존 루트로 식별한 테스트/entry point 가운데 실제로 연결된 것만 싣는다.
    pub review_roots: Vec<String>,
    pub unmapped_files: Vec<String>,
    pub truncation_reasons: Vec<&'static str>,
    pub limitations: Vec<String>,
    pub max_results: usize,
    pub max_seeds: usize,
    pub depth: usize,
}

/// 직접 수확한 내용의 fingerprint와 revision 라벨을 분리한다.
#[derive(Serialize)]
pub struct Provenance {
    pub origin: &'static str,
    pub revision: String,
    pub content_digest: String,
    pub freshness: &'static str,
    pub digest_scope: &'static str,
    pub project: String,
}

/// 파일 전체 seed임을 드러내어 변경 줄 정밀도로 오인하지 않게 한다.
#[derive(Serialize)]
pub struct Seed {
    pub id: String,
    pub file: String,
    pub reason: &'static str,
}

/// 현재 checkout은 건드리지 않고 로컬 object를 공유한 임시 clone에서 정확한 commit을 읽는다.
struct Checkout(PathBuf);
impl Checkout {
    fn new(repo: &Path, revision: &str) -> Result<Self, String> {
        let dir = std::env::temp_dir().join(format!(
            "rustograph-change-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        create_private_checkout(&dir).map_err(|e| format!("create isolated snapshot: {e}"))?;
        let owned = Self(dir);
        let result = guarded_git(repo)?
            .args([
                "clone",
                "--local",
                "--shared",
                "--no-checkout",
                "--template=",
                "--",
            ])
            .arg(git_path(repo))
            .arg(&owned.0)
            .output()
            .map_err(|e| format!("start git clone: {e}"))?;
        if !result.status.success() {
            return Err(format!(
                "capture Git snapshot: {}",
                String::from_utf8_lossy(&result.stderr)
            ));
        }
        git(
            &owned.0,
            &[
                "-c",
                "core.hooksPath=",
                "checkout",
                "--detach",
                revision,
                "--",
            ],
        )?;
        Ok(owned)
    }
    fn working(repo: &Path, revision: &str) -> Result<Self, String> {
        let checkout = Self::new(repo, revision)?;
        // index에서 사라진 staged 삭제/rename의 이전 경로도 HEAD 복사본에서 제거한다.
        let mut files: BTreeSet<String> = paths(&git(
            repo,
            &["ls-tree", "-r", "--name-only", "-z", revision, "--"],
        )?)?
        .into_iter()
        .collect();
        files.extend(paths(&git(
            repo,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )?)?);
        for file in files {
            let relative = Path::new(&file);
            let from = repo.join(relative);
            let to = checkout.0.join(relative);
            let destination = snapshot_metadata(&checkout.0, relative)?;
            match snapshot_metadata(repo, relative)? {
                Some(meta) if meta.is_file() => {
                    if destination.is_some_and(|m| m.file_type().is_symlink()) {
                        return Err(format!("snapshot source {file} changes a symlink; commit the regular input before analysis"));
                    }
                    if let Some(parent) = to.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
                    std::fs::copy(&from, &to).map_err(|e| format!("copy working source {file}: {e}"))?;
                }
                None => {
                    if destination.is_some_and(|m| m.is_file() || m.file_type().is_symlink()) { std::fs::remove_file(&to).map_err(|e| e.to_string())?; }
                }
                Some(_) => return Err(format!("working source {file} is a symlink/submodule/non-regular input; capture it explicitly before analysis")),
            }
        }
        Ok(checkout)
    }
}
impl Drop for Checkout {
    fn drop(&mut self) {
        // 생성에 성공한 고유 임시 디렉터리만 소유한다. 원본 checkout에는 쓰지 않는다.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// 복사 시작 전부터 소유자 외에는 소스를 공개하지 않는다. 기존 디렉터리 권한은 바꾸지 않는다.
#[cfg(unix)]
fn create_private_checkout(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new().mode(0o700).create(dir)
}

#[cfg(not(unix))]
fn create_private_checkout(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir(dir)
}

/// 정규화된 root 아래의 각 조상을 먼저 확인한다. 변경이 없는 트리의 경계 검사이며 원자적 snapshot은 아니다.
fn snapshot_metadata(root: &Path, relative: &Path) -> Result<Option<std::fs::Metadata>, String> {
    let components: Vec<_> = relative.components().collect();
    if components.is_empty()
        || components
            .iter()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err("Git returned an unsafe snapshot path".into());
    }
    let mut path = root.to_path_buf();
    for (index, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(format!(
                    "inspect snapshot input {}: {error}",
                    relative.display()
                ))
            }
        };
        if index + 1 == components.len() {
            return Ok(Some(metadata));
        }
        if metadata.file_type().is_symlink() {
            return Err(format!("snapshot input {} has a symlink ancestor; use regular in-repository inputs before analysis", relative.display()));
        }
        if !metadata.is_dir() {
            return Err(format!("snapshot input {} has a non-directory ancestor; restore the regular input before analysis", relative.display()));
        }
    }
    Ok(None)
}

/// base/head 또는 base/working-tree의 실제 변경을 수확한다. 실패를 빈 성공으로 바꾸지 않는다.
pub fn analyze(request: &Request<'_>) -> Result<Report, String> {
    if request.working_tree && request.head.is_some() {
        return Err("choose --head or --working-tree, not both".into());
    }
    if request.depth > 64 || request.max > 10_000 {
        return Err("depth must be 0..64 and max must be 0..10000".into());
    }
    let start = Instant::now();
    let dir = request
        .dir
        .canonicalize()
        .map_err(|e| format!("analysis directory: {e}"))?;
    let root = PathBuf::from(git_text(&dir, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let relative = dir
        .strip_prefix(&root)
        .map_err(|_| "analysis directory must be inside the Git repository")?;
    let base = revision(&root, request.base)?;
    let head = revision(&root, request.head.unwrap_or("HEAD"))?;
    let initial_working_digest = if request.working_tree {
        Some(content_digest(&root)?)
    } else {
        None
    };
    let mut args = vec![
        "diff",
        "--no-ext-diff",
        "--no-textconv",
        "--name-status",
        "-z",
        "--find-renames",
        base.as_str(),
    ];
    if !request.working_tree {
        args.push(head.as_str());
    }
    args.push("--");
    let mut changes = parse_changes(&git(&root, &args)?)?;
    if request.working_tree {
        for path in paths(&git(
            &root,
            &["ls-files", "--others", "--exclude-standard", "-z"],
        )?)? {
            changes.push(Change {
                status: "?".into(),
                old_path: None,
                new_path: Some(path),
            });
        }
    }
    changes.sort_by(|a, b| (&a.old_path, &a.new_path).cmp(&(&b.old_path, &b.new_path)));
    let before_checkout = Checkout::new(&root, &base)?;
    let before = capture(
        &before_checkout.0,
        relative,
        &base,
        false,
        &changes,
        true,
        request,
    )?;
    let after_checkout = if request.working_tree {
        Checkout::working(&root, &head)?
    } else {
        Checkout::new(&root, &head)?
    };
    let mut after = capture(
        &after_checkout.0,
        relative,
        &head,
        request.working_tree,
        &changes,
        false,
        request,
    )?;
    if let Some(initial) = initial_working_digest {
        if initial != content_digest(&root)? {
            after.provenance.freshness = "stale";
            after.limitations.push(
                "Working source changed while collecting the diff/snapshot; rerun the analysis."
                    .into(),
            );
            for impact in &mut after.impacts {
                impact.provenance.content_freshness = "stale";
            }
        }
    }
    let mut limitations = vec![
        "File-wide seeds are conservative review candidates, not changed-line precision.".into(),
        "Static graph coverage is incomplete: dynamic dispatch, callbacks, macro expansion and runtime configuration may add unseen paths; HTTP/data compatibility is not assessed.".into(),
        "Snapshots are harvested independently; before and after edges are never combined into a path.".into(),
        "Preparation includes local snapshot copies, Cargo metadata, content hashing and two fresh source harvests; no compilation or test execution is implied.".into(),
    ];
    if changes.iter().any(|c| {
        c.old_path
            .iter()
            .chain(c.new_path.iter())
            .any(|p| !p.ends_with(".rs"))
    }) {
        limitations.push("Changed non-Rust inputs may affect configuration, build or API/DB contracts outside the graph; they remain unmapped or trigger broad Cargo/build candidates.".into());
    }
    Ok(Report { mode: "git-change", state: "incomplete", changes, before, after, limitations,
        preparation_ms: start.elapsed().as_millis(),
        interpretation: "Review static candidates and run appropriate tests. Empty results do not establish safety or authorize skipping tests." })
}

fn capture(
    root: &Path,
    relative: &Path,
    revision: &str,
    working: bool,
    changes: &[Change],
    before: bool,
    request: &Request<'_>,
) -> Result<Snapshot, String> {
    let root_path = root.canonicalize().map_err(|e| e.to_string())?;
    let root = root_path.as_path();
    let project = root.join(relative);
    let first = content_digest(root)?;
    let options = source::Options {
        symbol_level: true,
        tests: true,
        cache: false,
        ..Default::default()
    };
    let context = source::CapturedRevision::new(root, &project)?;
    let (mut doc, inputs) = context.load_with_inputs(&options)?;
    doc.revision = Some(revision.to_owned());
    let second = content_digest(root)?;
    let mut limitations = doc.limitations.clone();
    let mut freshness = if first == second { "captured" } else { "stale" };
    if first != second {
        limitations.push(
            "Source content changed during capture; rerun before relying on candidates.".into(),
        );
    }
    if let Some(inputs) = inputs {
        let protected = inputs.protected_files();
        if protected.unresolved_includes
            || !protected.unloaded_modules.is_empty()
            || protected.files.iter().any(|p| !p.starts_with(root))
        {
            limitations.push("Input fingerprint does not prove external/generated/unresolved include inputs; their freshness is unknown.".into());
            if freshness == "captured" {
                freshness = "unknown";
            }
        }
    }
    let files: BTreeSet<String> = changes
        .iter()
        .filter_map(|c| {
            if before {
                c.old_path.clone()
            } else {
                c.new_path.clone()
            }
        })
        .collect();
    let broad = files.iter().any(|f| {
        f.ends_with("Cargo.toml")
            || f.ends_with("Cargo.lock")
            || f.ends_with("build.rs")
            || f.starts_with(".cargo/")
    });
    if broad {
        limitations.push("Cargo/build input changed: all harvested declarations are broad candidates; build-script and dependency behavior is not evaluated.".into());
    }
    let mut seeds: Vec<Seed> = doc
        .vertices
        .iter()
        .filter_map(|v| {
            if matches!(v.kind, Kind::Crate | Kind::Module) {
                return None;
            }
            let file = v
                .position
                .as_deref()
                .map(|p| position_file(p, root, &project))?;
            if broad || files.contains(&file) {
                Some(Seed {
                    id: v.id.clone(),
                    file,
                    reason: if broad {
                        "cargo-build-file-fallback"
                    } else {
                        "changed-file-declaration"
                    },
                })
            } else {
                None
            }
        })
        .collect();
    seeds.sort_by(|a, b| a.id.cmp(&b.id));
    seeds.dedup_by(|a, b| a.id == b.id);
    let mapped: BTreeSet<&str> = seeds.iter().map(|s| s.file.as_str()).collect();
    let unmapped_files = files
        .iter()
        .filter(|p| !mapped.contains(p.as_str()))
        .cloned()
        .collect();
    let mut truncation_reasons = Vec::new();
    if seeds.len() > MAX_SEEDS {
        truncation_reasons.push("max-seeds");
        seeds.truncate(MAX_SEEDS);
    }
    let index = analysis::QueryIndex::new(&doc);
    let mut remaining = request.max;
    let mut impacts = Vec::new();
    let mut review_roots = BTreeSet::new();
    for seed in &seeds {
        let mut result = index.impact(
            &seed.id,
            if request.depth == 0 {
                usize::MAX
            } else {
                request.depth
            },
            remaining,
        );
        result.depth = request.depth;
        result.mode = if before {
            "historical-snapshot"
        } else {
            "current-snapshot"
        };
        result.provenance.project = relative.to_string_lossy().replace('\\', "/");
        result.provenance.workspace = doc.workspace.as_deref().map(|workspace| {
            let path = Path::new(workspace);
            path.strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .replace('\\', "/")
        });
        result.provenance.origin = if working {
            "fresh-working-tree"
        } else {
            "fresh-git-snapshot"
        };
        result.provenance.content_freshness = freshness;
        remaining = remaining.saturating_sub(result.impacted.len());
        for row in &result.impacted {
            if doc.roots.contains(&row.id) {
                review_roots.insert(row.id.clone());
            }
        }
        if doc.roots.contains(&seed.id) {
            review_roots.insert(seed.id.clone());
        }
        for reason in &result.truncation_reasons {
            if !truncation_reasons.contains(reason) {
                truncation_reasons.push(reason);
            }
        }
        impacts.push(result);
    }
    Ok(Snapshot { provenance: Provenance { origin: if working { "working-tree" } else { "git-commit" }, revision: revision.into(),
        content_digest: second, freshness, digest_scope: "Git tracked and untracked non-ignored regular files; external/ignored generated inputs are not proven",
        project: relative.to_string_lossy().replace('\\', "/") }, seeds, impacts, review_roots: review_roots.into_iter().collect(),
        unmapped_files, truncation_reasons, limitations, max_results: request.max, max_seeds: MAX_SEEDS, depth: request.depth })
}

fn position_file(position: &str, root: &Path, project: &Path) -> String {
    let file = position
        .rsplit_once(':')
        .map(|(f, _)| f)
        .unwrap_or(position);
    let path = Path::new(file);
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project.join(path)
    };
    full.strip_prefix(root)
        .unwrap_or(&full)
        .to_string_lossy()
        .replace('\\', "/")
}

fn revision(root: &Path, value: &str) -> Result<String, String> {
    git_text(
        root,
        &[
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{value}^{{commit}}"),
        ],
    )
}
fn git_path(path: &Path) -> String {
    let text = path.to_string_lossy();
    if let Some(unc) = text.strip_prefix("\\\\?\\UNC\\") {
        format!("\\\\{unc}")
    } else {
        text.strip_prefix("\\\\?\\").unwrap_or(&text).to_owned()
    }
}
fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>, String> {
    let result = guarded_git(root)?
        .args(args)
        .output()
        .map_err(|e| format!("start git: {e}"))?;
    if !result.status.success() {
        return Err(format!(
            "Git input failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }
    Ok(result.stdout)
}
fn git_command() -> Result<Command, String> {
    // 체크아웃이 사용자의 필터·hook·fsmonitor 명령을 선택하지 않게 raw Git 내용을 쓴다.
    let search = std::env::var_os("PATH")
        .or_else(|| {
            if cfg!(unix) {
                Some("/bin:/usr/bin".into())
            } else {
                None
            }
        })
        .ok_or("Git requires an absolute PATH entry")?;
    let file = format!("git{}", std::env::consts::EXE_SUFFIX);
    let program = std::env::split_paths(&search)
        .filter(|p| p.is_absolute())
        .map(|p| p.join(&file))
        .find(|p| {
            let Ok(meta) = std::fs::metadata(p) else {
                return false;
            };
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            {
                meta.is_file()
            }
        })
        .ok_or("Git executable was not found in absolute PATH entries")?;
    let mut command = Command::new(program);
    command
        .args([
            "-c",
            "core.fsmonitor=false",
            "-c",
            "core.hooksPath=",
            "-c",
            "core.untrackedCache=false",
            "-c",
            "log.showSignature=false",
        ])
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env_remove("GIT_CONFIG")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    Ok(command)
}
fn guarded_git(root: &Path) -> Result<Command, String> {
    // 작업 트리를 읽지 않는 설정 조회부터 하고, diff/checkout 직전에 로컬 필터도 끈다.
    let keys = git_command()?
        .arg("-C")
        .arg(root)
        .args(["config", "-z", "--includes", "--name-only", "--list"])
        .output()
        .map_err(|e| format!("read Git filter configuration: {e}"))?;
    if !keys.status.success() {
        return Err("cannot inspect Git filter configuration".into());
    }
    let mut command = git_command()?;
    command.arg("-C").arg(root);
    for key in keys.stdout.split(|b| *b == 0) {
        if key.len() < 7 || !key[..7].eq_ignore_ascii_case(b"filter.") {
            continue;
        }
        let rest = &key[7..];
        let Some(dot) = rest.iter().rposition(|b| *b == b'.') else {
            continue;
        };
        if ![b"clean".as_slice(), b"smudge", b"process"]
            .iter()
            .any(|kind| rest[dot + 1..].eq_ignore_ascii_case(kind))
        {
            continue;
        }
        let name = std::str::from_utf8(&rest[..dot])
            .map_err(|_| "unsupported Git filter driver name; use a raw source checkout")?;
        if name.contains('=')
            || name
                .chars()
                .any(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'))
        {
            return Err("unsupported Git filter driver name; use a raw source checkout".into());
        }
        for setting in ["clean=", "smudge=", "process=", "required=false"] {
            command.arg("-c").arg(format!("filter.{name}.{setting}"));
        }
    }
    Ok(command)
}
fn git_text(root: &Path, args: &[&str]) -> Result<String, String> {
    String::from_utf8(git(root, args)?)
        .map(|s| s.trim().to_owned())
        .map_err(|_| "Git output is not UTF-8".into())
}
fn paths(bytes: &[u8]) -> Result<Vec<String>, String> {
    bytes
        .split(|b| *b == 0)
        .filter(|b| !b.is_empty())
        .map(|b| String::from_utf8(b.to_vec()).map_err(|_| "Git paths must be UTF-8".into()))
        .collect()
}
fn parse_changes(bytes: &[u8]) -> Result<Vec<Change>, String> {
    let parts = paths(bytes)?;
    let mut iter = parts.into_iter();
    let mut changes = Vec::new();
    while let Some(status) = iter.next() {
        let path = iter.next().ok_or("incomplete Git name-status record")?;
        let (old_path, new_path) = match status.as_bytes().first() {
            Some(b'R' | b'C') => (
                Some(path),
                Some(iter.next().ok_or("incomplete Git rename record")?),
            ),
            Some(b'A') => (None, Some(path)),
            Some(b'D') => (Some(path), None),
            _ => (Some(path.clone()), Some(path)),
        };
        changes.push(Change {
            status,
            old_path,
            new_path,
        });
    }
    Ok(changes)
}
fn content_digest(root: &Path) -> Result<String, String> {
    let mut files = paths(&git(
        root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
        ],
    )?)?;
    files.sort();
    files.dedup();
    let mut digest = Sha256::new();
    for file in files {
        let relative = Path::new(&file);
        let path = root.join(relative);
        digest.update(&(file.len() as u64).to_be_bytes());
        digest.update(file.as_bytes());
        match snapshot_metadata(root, relative)? {
            None => digest.update(b"missing"),
            Some(meta) if meta.is_file() => {
                use std::io::Read;
                let mut bytes = Vec::new();
                std::fs::File::open(&path)
                    .and_then(|f| f.take(source::MAX_SOURCE_BYTES + 1).read_to_end(&mut bytes))
                    .map_err(|e| format!("fingerprint {file}: {e}"))?;
                if bytes.len() as u64 > source::MAX_SOURCE_BYTES {
                    return Err(format!("fingerprint input {file} exceeds 64 MiB"));
                }
                digest.update(&(bytes.len() as u64).to_be_bytes());
                digest.update(&bytes);
            }
            Some(_) => digest.update(b"non-regular-input-not-captured"),
        }
    }
    Ok(digest.finalize_hex())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        root: PathBuf,
        repo: PathBuf,
    }
    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!(
                "rustograph-change-unit-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            let repo = root.join("repo");
            std::fs::create_dir_all(repo.join("src")).unwrap();
            std::fs::write(repo.join("src/lib.rs"), "pub fn original() {}\n").unwrap();
            std::fs::write(repo.join(".gitignore"), "/target/\n").unwrap();
            let fixture = Self { root, repo };
            fixture.git(&["init", "-q"]);
            fixture.git(&["add", "."]);
            fixture.git(&["commit", "-qm", "public fixture"]);
            fixture
        }
        fn git(&self, arguments: &[&str]) -> String {
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
                .arg(&self.repo)
                .args(arguments)
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
        #[cfg(unix)]
        fn ignored_ancestor_link(&self) {
            let outside = self.root.join("outside");
            std::fs::create_dir(&outside).unwrap();
            std::fs::write(outside.join("lib.rs"), "pub fn outside_only() {}\n").unwrap();
            std::fs::write(self.repo.join(".gitignore"), "/target/\n/src\n").unwrap();
            std::fs::remove_dir_all(self.repo.join("src")).unwrap();
            std::os::unix::fs::symlink(outside, self.repo.join("src")).unwrap();
            assert!(!self
                .git(&["ls-files", "--others", "--exclude-standard", "-z"])
                .split('\0')
                .any(|p| p == "src"));
            assert!(self
                .git(&["ls-files", "--cached", "-z"])
                .split('\0')
                .any(|p| p == "src/lib.rs"));
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[cfg(unix)]
    #[test]
    fn working_copy_rejects_an_ignored_symlink_ancestor() {
        let fixture = Fixture::new();
        let revision = fixture.git(&["rev-parse", "HEAD"]);
        fixture.ignored_ancestor_link();
        let result = Checkout::working(&fixture.repo, revision.trim());
        assert!(
            result.is_err(),
            "an ignored ancestor link must not copy outside source bytes"
        );
        assert!(result.err().unwrap().contains("symlink"));
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("outside/lib.rs")).unwrap(),
            "pub fn outside_only() {}\n"
        );
    }

    #[cfg(unix)]
    #[test]
    fn digest_rejects_an_ignored_symlink_ancestor() {
        let fixture = Fixture::new();
        fixture.ignored_ancestor_link();
        let result = content_digest(&fixture.repo);
        assert!(
            result.is_err(),
            "an ignored ancestor link must not fingerprint outside source bytes"
        );
        assert!(result.err().unwrap().contains("symlink"));
    }

    #[cfg(unix)]
    #[test]
    fn checkout_creation_is_private_and_cleans_success_and_failure_under_public_umask() {
        const CHILD: &str = "RUSTOGRAPH_CHANGE_PRIVATE_CHILD";
        if std::env::var_os(CHILD).is_none() {
            let fixture = Fixture::new();
            // umask는 이 테스트가 소유한 단일 자식에서만 바꾸며 부모 테스트·다른 프로세스에는 적용하지 않는다.
            let output = Command::new("/bin/sh")
                .args(["-c", "umask 022; exec \"$@\"", "rustograph-private-fixture"])
                .arg(std::env::current_exe().unwrap())
                .args(["--exact", "change::tests::checkout_creation_is_private_and_cleans_success_and_failure_under_public_umask", "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .env("TMPDIR", &fixture.root)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let revision = fixture.git(&["rev-parse", "HEAD"]);
        let checkout = Checkout::new(&fixture.repo, revision.trim()).unwrap();
        let path = checkout.0.clone();
        assert_eq!(
            std::fs::symlink_metadata(&path)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::read_to_string(path.join("src/lib.rs")).unwrap(),
            "pub fn original() {}\n"
        );
        drop(checkout);
        assert!(!path.exists());
        let prefix = format!("rustograph-change-{}-", std::process::id());
        assert!(Checkout::new(&fixture.repo, "not-a-revision").is_err());
        assert!(!std::fs::read_dir(std::env::temp_dir())
            .unwrap()
            .any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&prefix)));
    }
}
