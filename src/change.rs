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
        std::fs::create_dir(&dir).map_err(|e| format!("create isolated snapshot: {e}"))?;
        let owned = Self(dir);
        let result = Command::new("git")
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
        for file in paths(&git(
            repo,
            &[
                "ls-files",
                "--cached",
                "--others",
                "--exclude-standard",
                "-z",
            ],
        )?)? {
            let relative = Path::new(&file);
            if relative
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
            {
                return Err("Git returned an unsafe snapshot path".into());
            }
            let from = repo.join(relative);
            let to = checkout.0.join(relative);
            match std::fs::symlink_metadata(&from) {
                Ok(meta) if meta.is_file() => {
                    if let Some(parent) = to.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
                    std::fs::copy(&from, &to).map_err(|e| format!("copy working source {file}: {e}"))?;
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    if to.is_file() { std::fs::remove_file(&to).map_err(|e| e.to_string())?; }
                }
                Err(e) => return Err(format!("read working source {file}: {e}")),
                Ok(_) => return Err(format!("working source {file} is a symlink/submodule/non-regular input; capture it explicitly before analysis")),
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
    let (mut doc, inputs) = source::load_with_inputs(&project, &options)?;
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
    let result = Command::new("git")
        .arg("-C")
        .arg(root)
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
        let path = root.join(&file);
        digest.update(&(file.len() as u64).to_be_bytes());
        digest.update(file.as_bytes());
        match std::fs::symlink_metadata(&path) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => digest.update(b"missing"),
            Err(e) => return Err(format!("fingerprint {file}: {e}")),
            Ok(meta) if meta.is_file() => {
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
            Ok(_) => digest.update(b"non-regular-input-not-captured"),
        }
    }
    Ok(digest.finalize_hex())
}
