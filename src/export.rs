//! 문서 출력 — 결정적 JSON, Mermaid, 저장/읽기.
//!
//! serde_json은 필드 선언 순서로 직렬화하고 BTreeMap은 정렬된 키를 낸다 —
//! 같은 입력이 같은 바이트가 되는 계약은 struct 필드 순서와 정렬된 Vec에 있다.

use crate::graph::Document;
use std::collections::BTreeSet;
use std::io::Write;
use std::path::{Path, PathBuf};

/// 문서를 결정적 pretty JSON으로 직렬화한다.
pub fn to_json<T: serde::Serialize>(v: &T) -> String {
    let mut s = serde_json::to_string_pretty(v).expect("document must serialize");
    s.push('\n');
    s
}

/// Mermaid 그래프 — 사람이 보는 용도. 정점 ID가 라벨이 되고, 렌더된 라벨은 ID 그대로다.
pub fn to_mermaid(doc: &Document) -> String {
    let mut out = String::from("graph TD\n");
    let ids: std::collections::BTreeSet<&str> = doc.vertex_ids();
    for e in &doc.edges {
        if !ids.contains(e.from.as_str()) || !ids.contains(e.to.as_str()) {
            continue;
        }
        out.push_str(&mermaid_edge(e));
    }
    out
}

/// 간선 한 줄. 엔터티 코드를 쓴 줄은 `;`(문장 끝)로 맺는다 — Mermaid는 파싱 전에 `style`·
/// `classDef`가 든 줄에서 `:` 뒤 `#…;`를 색 값으로 보고 그 줄의 마지막 `;`를 지운다.
/// 덧붙인 `;`가 없으면 마지막 엔터티(`#gt;`)의 `;`가 지워져 `#gt`가 글자로 남는다.
fn mermaid_edge(e: &crate::graph::Edge) -> String {
    let (from, to) = (mermaid_label(&e.from), mermaid_label(&e.to));
    let end = if from != e.from || to != e.to {
        ";"
    } else {
        ""
    };
    let label = if e.tentative {
        format!("{} (candidate)", edge_label(e.kind))
    } else {
        edge_label(e.kind).to_string()
    };
    format!(
        "    {}[\"{from}\"] -->|{}| {}[\"{to}\"]{end}\n",
        node_id(&e.from),
        label,
        node_id(&e.to)
    )
}

/// 따옴표 라벨 텍스트. `"`는 라벨을 닫아 뒤에 문장(`;click … href`)을 끼울 수 있고, `%`는
/// 라벨 안에서도 감지되는 `%%{init: …}%%` 지시문을 열며, `<`·`>`·`&`는 HTML 라벨의 태그·
/// 엔터티로 읽힌다(트레이트 impl ID `Type::<Trait>::m`의 `<Trait>`가 태그로 사라졌다).
/// Mermaid 엔터티 코드는 렌더할 때 그 글자로 돌아온다. 수확 ID에 없는 `%`·`&`·`"`는
/// 다른 도구가 만든 `--graph` 문서에서만 바뀐다.
fn mermaid_label(id: &str) -> String {
    let mut label = String::with_capacity(id.len());
    for c in id.chars() {
        match c {
            '"' => label.push_str("#quot;"),
            '%' => label.push_str("#37;"),
            '<' => label.push_str("#lt;"),
            '>' => label.push_str("#gt;"),
            '&' => label.push_str("#amp;"),
            c => label.push(c),
        }
    }
    label
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

/// 스냅샷이 가리키는 원본 소스를 산출물로 바꾸지 않도록 보호한다. 소스 파일은 처음 나온
/// 순서대로 한 번씩만 싣는다 — 정점 대부분이 몇 안 되는 파일을 공유해(실제 수확 9,394정점이
/// 파일 271개), 정점마다 넣으면 출력을 덮어쓸 때 같은 경로를 정점 수만큼 canonicalize한다.
pub(crate) fn document_inputs(doc: &Document) -> Vec<PathBuf> {
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    let mut inputs: Vec<PathBuf> = doc
        .vertices
        .iter()
        .filter_map(|vertex| vertex.position.as_deref())
        .filter_map(|position| position.rsplit_once(':'))
        .filter(|(file, _)| seen.insert(*file))
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

#[cfg(test)]
thread_local! {
    /// 출력 검사가 보호 입력을 canonicalize한 횟수 — 같은 입력을 정점 수만큼 거듭 풀지 않는지
    /// 단위 테스트가 센다(테스트 스레드마다 따로).
    static INPUT_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 테스트용 — 이 스레드에서 지금까지 출력 검사가 보호 입력을 canonicalize한 횟수.
#[cfg(test)]
pub(crate) fn input_probes() -> usize {
    INPUT_PROBES.with(std::cell::Cell::get)
}

/// 보호 경로를 처음 나온 순서대로 한 번씩만 남긴다. 호출자가 합친 목록(문서 정점 위치·수확
/// 입력·설정)은 같은 파일을 겹쳐 싣는데, 겹친 경로는 같은 판정을 되풀이할 뿐이다. 순서는 지킨다 —
/// 살피지 못한 입력 중 처음 것이 오류에 실린다.
fn distinct_paths(paths: &[PathBuf]) -> Vec<&Path> {
    let mut seen: BTreeSet<&Path> = BTreeSet::new();
    paths
        .iter()
        .map(PathBuf::as_path)
        .filter(|input| seen.insert(*input))
        .collect()
}

fn validate_output(path: &Path, protected: &[&Path]) -> Result<(), String> {
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
        #[cfg(test)]
        INPUT_PROBES.with(|c| c.set(c.get() + 1));
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
    let protected = distinct_paths(protected);
    validate_output(path, &protected)?;
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
        validate_output(&destination, &protected)?;
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
/// ID 필드에 제어 문자·U+2028/2029가 있으면 거부한다 — 다른 도구가 만든 문서의 그런 ID는
/// cycles·dead·rules 텍스트로 터미널을 조작하고 Mermaid 문장을 끊는다. 오류에는 문자 대신
/// 코드 포인트를 싣는다(오류도 터미널에 나간다).
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
    if let Some((field, c)) = doc.forbidden_id_char() {
        return Err(format!(
            "bad {}: {field} contains U+{:04X}; ids may not hold control characters or U+2028/U+2029 — regenerate the document with `rustograph graph --level symbol --out <file>`",
            path.display(),
            u32::from(c)
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

    /// 보호 목록은 소스 파일을 정점마다가 아니라 한 번씩만 싣는다 — 정점 위치를 그대로 넣으면
    /// 실제 수확(9,394정점이 파일 271개)에서 같은 경로가 정점 수만큼 겹쳐, 출력을 덮어쓸 때마다
    /// 그 수의 두 배를 canonicalize했다. 처음 나온 순서와 경로 집합은 그대로다.
    #[test]
    fn document_inputs_list_each_source_file_once() {
        let fn_at = |id: &str, position: &str| Vertex {
            id: id.into(),
            kind: Kind::Fn,
            krate: "c".into(),
            module: "c::m".into(),
            position: Some(position.into()),
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        };
        let mut vertices = doc().vertices;
        vertices.extend([
            fn_at("c::m::a", "src/m.rs:3"),
            fn_at("c::m::b", "src/m.rs:9"),
            fn_at("c::n", "src/n.rs:1"),
            fn_at("c::n::f", "src/n.rs:4"),
        ]);
        let d = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            vertices,
            vec![],
            vec![],
        );
        let expected: Vec<PathBuf> = [
            "src/m.rs",
            "src/n.rs",
            "Cargo.toml",
            "Cargo.lock",
            ".rustograph.yml",
            ".rustograph.yaml",
        ]
        .into_iter()
        .map(|file| Path::new(".").join(file))
        .collect();
        assert_eq!(document_inputs(&d), expected);
    }

    /// 기존 출력을 바꿀 때 보호 입력은 검사마다 서로 다른 경로당 한 번만 canonicalize한다 —
    /// 겹친 경로는 같은 결과를 다시 계산할 뿐이다. 새 경로는 입력의 별칭일 수 없어 입력을 풀지
    /// 않는다. 별칭 거부와, 살피지 못한 입력 중 처음 나온 것을 오류에 싣는 순서는 그대로다.
    #[test]
    fn replacing_an_output_resolves_each_protected_input_once_per_check() {
        let dir = std::env::temp_dir().join(format!("rg-input-probes-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let (a, b) = (dir.join("a.rs"), dir.join("b.rs"));
        std::fs::write(&a, "a").unwrap();
        std::fs::write(&b, "b").unwrap();
        let out = dir.join("out.json");
        std::fs::write(&out, "previous").unwrap();
        let probed = |path: &Path, protected: &[PathBuf]| {
            let before = input_probes();
            let result = write_file(path, b"replacement", protected);
            (result, input_probes() - before)
        };
        let overlapping = [
            a.clone(),
            a.clone(),
            b.clone(),
            a.clone(),
            dir.join("missing.rs"),
        ];
        let (result, replacing) = probed(&out, &overlapping);
        result.unwrap();
        // 공개 전 검사와 rename 직전 재검사 × 서로 다른 입력 셋.
        assert_eq!(replacing, 2 * 3);
        let (result, fresh) = probed(&dir.join("fresh.json"), &overlapping);
        result.unwrap();
        assert_eq!(fresh, 0);
        let (result, _) = probed(&out, &[a.clone(), a.clone(), out.clone()]);
        assert!(result.unwrap_err().contains("aliases an input"));
        assert_eq!(std::fs::read(&out).unwrap(), b"replacement");
        #[cfg(unix)]
        {
            // 파일을 디렉터리처럼 지나는 경로는 NotFound가 아닌 오류다 — 처음 나온 입력을 싣는다.
            let (first, second) = (b.join("x"), a.join("y"));
            let (result, _) = probed(&out, &[first.clone(), second, first.clone()]);
            let error = result.unwrap_err();
            let named = format!("cannot inspect input {}:", first.display());
            assert!(error.starts_with(&named), "{error}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
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

    /// 다른 도구가 만든 문서의 ID에 제어 문자가 섞이면 텍스트 출력이 터미널을 조작하고
    /// Mermaid 문장이 끊긴다 — 읽을 때 필드·코드 포인트와 해결 방향을 담아 거부한다.
    /// 오류 문구는 그 문자를 그대로 싣지 않는다(오류도 터미널에 나간다).
    #[test]
    fn load_rejects_ids_with_control_characters() {
        let dir = std::env::temp_dir().join(format!("rg-export3-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("g.json");
        let mut tainted = doc();
        tainted.edges[0].to = "c::m\u{1b}[2J".into();
        std::fs::write(&p, to_json(&tainted)).unwrap();
        let err = load_file(&p).unwrap_err();
        assert!(err.contains("edges[0].to contains U+001B"), "{err}");
        assert!(
            err.contains("rustograph graph --level symbol --out"),
            "{err}"
        );
        assert!(!err.contains('\u{1b}'), "{err:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn mermaid_has_nodes_and_edges() {
        let m = to_mermaid(&doc());
        assert!(m.contains("graph"));
        assert!(m.contains("c__m"));
        assert!(m.contains("contains"));
    }

    /// Mermaid 따옴표 라벨은 Mermaid에서 뜻을 갖는 글자를 엔터티 코드로 바꾼다 — 트레이트 impl
    /// ID의 `<Tr>`가 HTML 태그로 사라지지 않고, 다른 도구 문서의 `"`·`%%{`가 문장·지시문을
    /// 끼우지 못한다. 엔터티를 쓴 줄은 `;`로 끝나고(`style`이 든 줄의 마지막 `;`를 Mermaid가
    /// 지운다), 바꿀 글자가 없는 줄은 그대로다.
    #[test]
    fn mermaid_labels_escape_markup_and_directives() {
        let fn_vertex = |id: &str| Vertex {
            id: id.into(),
            kind: Kind::Fn,
            krate: "c".into(),
            module: "c".into(),
            position: None,
            exported: false,
            generated: false,
            cfg: None,
            unsafe_: false,
        };
        let quote = r#"c::q"];click c href "javascript:alert(1)";x[""#;
        let directive = "c::%%{init: {'theme':'dark'}}%%";
        let ids = [
            "c",
            "c::T",
            "c::T::<Tr>::m",
            "c::a&b",
            "c::style::f",
            quote,
            directive,
        ];
        let edges = [
            ("c", "c::T", EdgeKind::Contains),
            ("c::T", "c::T::<Tr>::m", EdgeKind::Contains),
            ("c::style::f", "c::T::<Tr>::m", EdgeKind::Call),
            ("c", quote, EdgeKind::Contains),
            ("c", directive, EdgeKind::Contains),
            ("c", "c::a&b", EdgeKind::Contains),
        ];
        let d = document(
            Level::Symbol,
            ".".into(),
            None,
            vec![],
            ids.into_iter().map(fn_vertex).collect(),
            edges
                .into_iter()
                .map(|(from, to, k)| Edge::new(from.into(), to.into(), k))
                .collect(),
            vec![],
        );
        let expected = concat!(
            "graph TD\n",
            "    c[\"c\"] -->|contains| c_____init____theme___dark_____[\"c::#37;#37;{init: {'theme':'dark'}}#37;#37;\"];\n",
            "    c[\"c\"] -->|contains| c__T[\"c::T\"]\n",
            "    c[\"c\"] -->|contains| c__a_b[\"c::a#amp;b\"];\n",
            "    c[\"c\"] -->|contains| c__q___click_c_href__javascript_alert_1___x__[\"c::q#quot;];click c href #quot;javascript:alert(1)#quot;;x[#quot;\"];\n",
            "    c__T[\"c::T\"] -->|contains| c__T___Tr___m[\"c::T::#lt;Tr#gt;::m\"];\n",
            "    c__style__f[\"c::style::f\"] -->|call| c__T___Tr___m[\"c::T::#lt;Tr#gt;::m\"];\n",
        );
        assert_eq!(to_mermaid(&d), expected);
    }
}
