//! 문서 출력 — 결정적 JSON, Mermaid, 저장/읽기.
//!
//! serde_json은 필드 선언 순서로 직렬화하고 BTreeMap은 정렬된 키를 낸다 —
//! 같은 입력이 같은 바이트가 되는 계약은 struct 필드 순서와 정렬된 Vec에 있다.

use crate::graph::Document;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 문서를 결정적 pretty JSON으로 직렬화한다.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("document must serialize");
    s.push('\n');
    s
}

/// Mermaid 그래프 — 사람이 보는 용도. 정점 ID는 그대로 라벨이 된다.
pub fn to_mermaid(doc: &Document) -> String {
    let mut out = String::from("graph TD\n");
    let ids: std::collections::BTreeSet<&str> = doc.vertex_ids();
    for e in &doc.edges {
        if !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()) {
            continue;
        }
        out.push_str(&format!(
            "    {}[\"{}\"] -->|{}| {}[\"{}\"]\n",
            node_id(&e.from),
            e.from,
            edge_label(e.kind),
            node_id(&e.to),
            e.to
        ));
    }
    out
}

fn node_id(id: &str) -> String {
    // mermaid 노드 ID에 쓸 수 없는 문자를 밑줄로.
    id.chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .collect()
}

fn edge_label(kind: crate::graph::EdgeKind) -> &'static str {
    use crate::graph::EdgeKind::*;
    match kind {
        Depends => "depends",
        Uses => "uses",
        Contains => "contains",
        Call => "call",
        References => "references",
        Implements => "implements",
        Signature => "signature",
    }
}

/// 문서를 파일로 저장한다.
pub fn save_file(doc: &Document, path: &Path) -> Result<(), String> {
    let protected = document_inputs(doc);
    write_file(path, to_json(doc).as_bytes(), &protected)
}

/// 스냅샷이 가리키는 원본 소스를 산출물로 바꾸지 않도록 보호한다.
pub(crate) fn document_inputs(doc: &Document) -> Vec<PathBuf> {
    let mut inputs: Vec<PathBuf> = doc
        .vertices
        .iter()
        .filter_map(|vertex| vertex.position.as_ref())
        .filter_map(|position| position.rsplit_once(':'))
        .map(|(file, _)| Path::new(&doc.root).join(file))
        .collect();
    inputs.extend(
        [
            "Cargo.toml",
            "Cargo.lock",
            ".rustograph.yml",
            ".rustograph.yaml",
        ]
        .into_iter()
        .map(|name| Path::new(&doc.root).join(name)),
    );
    inputs
}

/// 완성된 바이트만 같은 디렉터리의 rename으로 공개한다. 입력 별칭은 거절한다.
pub fn write_file(path: &Path, contents: &[u8], protected: &[PathBuf]) -> Result<(), String> {
    write_file_with(path, protected, |file| file.write_all(contents))
}

fn validate_output(path: &Path, protected: &[PathBuf]) -> Result<(), String> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(format!("cannot inspect {}: {error}", path.display())),
    };
    if !metadata.is_file() || metadata.permissions().readonly() {
        return Err(format!(
            "refusing non-regular, symbolic-link or read-only output {}",
            path.display()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.nlink() > 1 {
            return Err(format!("refusing hard-linked output {}", path.display()));
        }
    }
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", path.display()))?;
    for input in protected {
        match input.canonicalize() {
            Ok(input) if input == resolved => {
                return Err(format!(
                    "output {} aliases an input; choose a different output path",
                    path.display()
                ))
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("cannot inspect input {}: {error}", input.display())),
        }
    }
    Ok(())
}

fn write_file_with(
    path: &Path,
    protected: &[PathBuf],
    write: impl FnOnce(&mut std::fs::File) -> std::io::Result<()>,
) -> Result<(), String> {
    validate_output(path, protected)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create {}: {error}", parent.display()))?;
    let parent = parent
        .canonicalize()
        .map_err(|error| format!("cannot resolve {}: {error}", parent.display()))?;
    let destination = parent.join(path.file_name().ok_or("output must name a file")?);
    static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let (temporary, mut file) = loop {
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = parent.join(format!(".rustograph-{}-{sequence}.tmp", std::process::id()));
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("cannot stage {}: {error}", path.display())),
        }
    };
    let result = (|| {
        write(&mut file).map_err(|error| format!("cannot stage {}: {error}", path.display()))?;
        file.sync_all()
            .map_err(|error| format!("cannot sync {}: {error}", path.display()))?;
        // 준비 중 교체된 입력 별칭도 공개 직전에 다시 확인한다.
        validate_output(&destination, protected)?;
        std::fs::rename(&temporary, &destination)
            .map_err(|error| format!("cannot publish {}: {error}", path.display()))
    })();
    if let Err(failure) = result {
        if let Err(error) = std::fs::remove_file(&temporary) {
            return Err(format!(
                "{failure}; cannot remove staged file {}: {error}",
                temporary.display()
            ));
        }
        return Err(failure);
    }
    Ok(())
}

/// 저장된 문서를 읽는다. 미래 버전은 거부한다 — 모르는 필드보다 버전 불일치가 위험하다.
pub fn load_file(path: &Path) -> Result<Document, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    let doc: Document =
        serde_json::from_str(&src).map_err(|e| format!("bad {}: {e}", path.display()))?;
    if doc.version > crate::graph::DOCUMENT_VERSION {
        return Err(format!(
            "graph document version {} is newer than this tool understands ({})",
            doc.version,
            crate::graph::DOCUMENT_VERSION
        ));
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{document, Edge, EdgeKind, Kind, Level, Vertex};

    fn doc() -> Document {
        document(
            Level::Module,
            ".".into(),
            None,
            vec!["c".into()],
            vec![
                Vertex {
                    id: "c".into(),
                    kind: Kind::Crate,
                    krate: "c".into(),
                    module: "c".into(),
                    position: None,
                    exported: true,
                    generated: false,
                    cfg: None,
                    unsafe_: false,
                },
                Vertex {
                    id: "c::m".into(),
                    kind: Kind::Module,
                    krate: "c".into(),
                    module: "c::m".into(),
                    position: Some("src/m.rs:1".into()),
                    exported: false,
                    generated: false,
                    cfg: None,
                    unsafe_: false,
                },
            ],
            vec![Edge::new("c".into(), "c::m".into(), EdgeKind::Contains)],
            vec![],
        )
    }

    #[test]
    fn failed_stage_preserves_previous_output_and_removes_temporary() {
        let dir = std::env::temp_dir().join(format!("rg-stage-failure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("result.json");
        std::fs::write(&path, b"previous complete output").unwrap();
        let failure = write_file_with(&path, &[], |file| {
            file.write_all(b"partial replacement")?;
            Err(std::io::Error::other("injected write failure"))
        })
        .unwrap_err();
        assert!(failure.contains("injected write failure"));
        assert_eq!(std::fs::read(&path).unwrap(), b"previous complete output");
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        write_file(&path, b"new complete output", &[]).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"new complete output");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn json_is_deterministic() {
        let a = to_json(&doc());
        let b = to_json(&doc());
        assert_eq!(a, b);
        // 정점·간선 배열이 id/from/to/kind 순으로 정렬돼 있다 — 같은 입력이
        // 같은 바이트여야 diff·캐시가 성립한다.
        let j: serde_json::Value = serde_json::from_str(&a).unwrap();
        let ids: Vec<&str> = j["vertices"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["id"].as_str().unwrap())
            .collect();
        let mut sorted = ids.clone();
        sorted.sort();
        assert_eq!(ids, sorted);
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = std::env::temp_dir().join(format!("rg-export-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.json");
        save_file(&doc(), &p).unwrap();
        let loaded = load_file(&p).unwrap();
        assert_eq!(to_json(&loaded), to_json(&doc()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_rejects_future_version() {
        let dir = std::env::temp_dir().join(format!("rg-export2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.json");
        std::fs::write(&p, r#"{"version":999,"tool":"rustograph","level":"module","root":".","roots":[],"vertices":[],"edges":[]}"#).unwrap();
        assert!(load_file(&p).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn mermaid_has_nodes_and_edges() {
        let m = to_mermaid(&doc());
        assert!(m.contains("graph"));
        assert!(m.contains("c__m"));
        assert!(m.contains("contains"));
    }
}
