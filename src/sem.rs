//! 의미 해석 엔진 — ra_ap(rust-analyzer) 기반.
//!
//! `semantic` cargo feature로만 컴파일된다 — 의존이 크고 주 단위 API 변동이
//! 있어 opt-in이다. syn 수확이 이름으로 추정하던 부분을 실제 타입 해석으로
//! 대체한다: 메서드 호출은 수신자 타입으로 확정하고, dyn·제네릭 수신자의
//! 트레이트 호출은 워크스페이스 impl 후보로 추정 간선을 펼치며(trait impl
//! 행렬), 매크로 호출은 확장 트리까지 내려가 내부의 호출·참조를 수확한다.
//! 그래프 계약(정점·간선 스키마, tentative 의미)은 바꾸지 않는다 —
//! 정확도만 올리는 것이 이 모듈의 존재 이유다.
//!
//! 경계 규칙: ra_ap은 여기서만 만진다. 구조(정점·contains·uses·implements·
//! signature)는 전부 syn 수확이 권위다 — 이 모듈은 본문 간선만 낸다.

use crate::cargo_meta::normalize_name;
use crate::graph::{Edge, EdgeKind};
use ra_ap_hir::db::HirDatabase;
use ra_ap_hir::{
    Adt, AsAssocItem, AssocItem, AssocItemContainer, Const, Crate, Enum, Function, Impl, InFile,
    Macro, Module, ModuleDef, PathResolution, Semantics, Static, Trait, Type, TypeAlias,
};
use ra_ap_ide_db::RootDatabase;
use ra_ap_load_cargo::{load_workspace_at, LoadCargoConfig, ProcMacroServerChoice};
use ra_ap_proc_macro_api::ProcMacroClient;
use ra_ap_project_model::{CargoConfig, RustLibSource, TargetDirectoryConfig};
use ra_ap_syntax::ast::{self, AstNode, HasAttrs, HasName};
use ra_ap_syntax::{SyntaxNode, TextRange, TextSize};
use ra_ap_vfs::{FileId, Vfs, VfsPath};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};

/// 매크로 확장 재귀 한계 — 재귀 매크로(`m!() => { m!() }`류)는 유한해야 한다.
const MAX_EXPANSION_DEPTH: usize = 16;

/// 의미 해석 실측 — 호출자가 limitation 문장으로 번역한다.
#[derive(Debug, Default)]
pub struct Stats {
    /// 타입 정보로 확정한 호출·참조 간선 수(추정이 아닌 것).
    pub resolved: usize,
    /// 트레이트 디스패치 지점 수 — dyn/제네릭 수신자라 impl 후보로 펼쳤다.
    pub trait_sites: usize,
    /// ra도 해석하지 못해 이름 팬아웃으로 되돌아간 메서드 호출 수.
    pub fanned: usize,
    /// 그래프 밖(의존 크레이트·std·derive 생성 정의)으로 해석된 타깃 수.
    pub external: usize,
    /// 해석된 외부·std 매크로 호출 수 — syn의 external_macros와 같은 버킷.
    pub ext_macros: usize,
    /// 그중 proc 매크로 호출 수 — 서버가 없으면 확장이 불가하므로
    /// limitation에서 원인을 별도로 짚는다.
    pub proc_macros: usize,
    /// 확장에 실패한 매크로 호출 수(proc 서버 부재·확장 오류).
    pub unexpanded: usize,
    /// 확장된 매크로 호출 수 — limitation이 아니라 커버리지 지표.
    pub expanded: usize,
    /// ra도 해석 못 한 경로 참조 수 — syn의 unresolved_paths와 같은 버킷.
    pub unresolved: usize,
    /// hir이 모르는 본문 수 — cfg 비활성·매크로 생성 정의는 syn 폴백으로 간다.
    pub unmapped: usize,
    /// 디스패치 후보 중 그래프에 표현 불가인 것의 수 — blanket·원시 타입
    /// impl은 메서드 정점도 ADT owner도 없어 간선이 아니라 여기로 센다.
    pub unrepresentable: usize,
}

/// 의미 해석 세션 — 로드된 워크스페이스 DB와 정규 ID → 정의 인덱스.
/// DB는 salsa 스냅샷이다 — load 이후 소스가 바뀌면 다시 load해야 한다.
pub struct Engine {
    db: RootDatabase,
    /// vfs는 본문 소유자의 소스 정체(파일)를 맞출 때 쓴다 — 정규 ID만으로는
    /// cfg 변형·lib/bin 합본을 구분 못 한다. proc 매크로 서버는 외부
    /// 프로세스라 클라이언트를 살려둬야 확장이 동작한다.
    vfs: Vfs,
    proc_macro: Option<ProcMacroClient>,
    /// 정규 ID → 본문 소유자 정의(fn·const·static)와 소스 정체.
    /// 워크스페이스 크레이트만, 블록 지역 정의는 제외. 같은 ID의 정의가
    /// 여럿일 수 있어(`impl S<u8>`/`S<u16>`) 항목은 벡터다.
    defs: BTreeMap<String, Vec<BodyEntry>>,
    /// 정규 ID → syn이 수확한 선언 위치들(파일·바이트 범위·트레이트) —
    /// hir 정의의 정점 provenance 검증에 쓴다. 문자열 ID만으로는 생성
    /// 메서드가 같은 이름의 진짜 정점과 충돌하는 것을 구분 못 한다.
    sites: BTreeMap<String, Vec<SynSite>>,
    /// 크레이트 스코프에 보이는 impl들 — `Impl::all_in_crate`가 익명
    /// const 블록까지 따라가 수집한 집합이다. fn 본문 안 블록 모듈의
    /// impl은 여기 없으므로 지역 정의와 모듈 정의를 구분하는 열쇠다.
    impls: HashSet<Impl>,
    /// (트레이트 정규 ID, 메서드 이름) → 워크스페이스 impl 후보들.
    /// 디스패치 지점마다 impl을 다시 훑지 않게 빌드 때 한 번 만든다.
    matrix: BTreeMap<(String, String), Vec<Candidate>>,
}

/// syn이 수확한 선언 위치 한 건 — provenance 검증의 대조 대상.
struct SynSite {
    /// 선언이 수확된 파일.
    file: FileId,
    /// 아이템 전체의 바이트 범위.
    range: TextRange,
    /// 선언이 속한 모듈의 정규 경로 — 같은 물리 파일을 여러 모듈이
    /// 가리킬 때(`#[path]`·공유 파일) ra가 임의의 문맥으로 def를 묶을
    /// 수 있으므로 해석된 def의 소유 모듈과 대조한다.
    module: String,
    /// 사이트 선언이 ra에서 가리키는 정의 — salsa intern ID라 같은
    /// def면 같은 아이템이다: 렌더링된 문자열이 아니므로 `a::Tr`/`b::Tr`·
    /// `G<u8>`/`G<u16>`·타입 별칭 인자의 구분이 공짜로 따라온다.
    /// 해석 불가·애매(속성 매크로 사본이 여러 def로 갈림)·모듈 문맥
    /// 불일치면 None — 위치만으로는 span을 재사용한 생성 정의와 구분이
    /// 안 되므로 매칭하지 않고 syn 폴백으로 돌린다.
    def: Option<BodyDef>,
}

/// syn 측 선언 위치 입력 — 엔진이 vfs 좌표로 변환해 보관한다.
pub struct Site {
    /// 선언이 수확된 파일.
    pub file: PathBuf,
    /// 아이템 전체의 바이트 범위.
    pub range: Range<usize>,
    /// 선언이 속한 모듈의 정규 경로 — `crate::a::b` 형태.
    pub module: String,
}

/// 본문을 가질 수 있는 정의 — fn·const·static.
/// provenance는 def 정체 자체로 비교한다 — hir 정의는 intern된 ID라
/// 동등 비교가 곧 같은 아이템 판정이다.
#[derive(Clone, Copy, PartialEq, Eq)]
enum BodyDef {
    Fn(Function),
    Const(Const),
    Static(Static),
}

/// 인덱스 항목 — 정의 + 소스 정체. 같은 정규 ID를 가리키는 정의가
/// 여럿일 수 있어(cfg 변형 등) 파일·범위로 진짜 소유자를 가린다.
struct BodyEntry {
    def: BodyDef,
    /// 정의가 속한 크레이트 — 같은 파일·범위가 여러 크레이트 문맥으로
    /// 로드될 때(include! 공유) 진짜 소유자를 가린다.
    krate: String,
    file: FileId,
    /// 아이템 전체의 원본 파일 범위.
    range: TextRange,
    /// 이름 토큰의 원본 범위 — 주석·속성 포함 범위는 syn과 ra가 다르게
    /// 나눌 수 있지만 식별자 위치는 양쪽이 같은 선언을 가리킨다.
    /// `const _` 같은 무명 정의는 없다.
    anchor: Option<TextRange>,
}

/// 디스패치 후보 — 메서드 정점을 우선 쓰고, 정점이 없는 생성 impl이면
/// impl 대상 타입 정점으로 폴백한다. 속성 매크로가 감싼 impl은
/// 확장 파일 소스를 가져도 메서드 정점이 실재하므로 method가 먼저다.
struct Candidate {
    /// `T::m`·`T::<Tr>::m` 정규 ID — 생성 impl이면 정점이 없을 수 있다.
    method: Option<String>,
    /// impl 대상 타입 정점 — 생성 impl에서만 채우는 폴백.
    owner: Option<String>,
}

/// 빌드 결과 — 본문 소유자 인덱스 + 트레이트 디스패치 후보 표.
struct Index {
    /// 정규 ID → 본문 소유자 정의와 소스 정체 — 같은 ID의 정의가
    /// 여럿일 수 있어 항목은 벡터다.
    defs: BTreeMap<String, Vec<BodyEntry>>,
    /// 크레이트 스코프 impl 집합 — fn 블록 모듈의 impl은 없다.
    impls: HashSet<Impl>,
    /// (트레이트 정규 ID, 메서드 이름) → 워크스페이스 impl 후보들.
    matrix: BTreeMap<(String, String), Vec<Candidate>>,
}

/// 본문 소유자 항목 — 정규 ID + 소스 정체(파일·바이트 범위) + cfg.
/// 정규 ID만으로는 cfg 변형·lib/bin 합본을 구분 못 해 소스 정체까지 받는다.
pub struct OwnerSite<'a> {
    /// 정규 정점 ID — syn 수확과 같은 스킴.
    pub id: &'a str,
    /// 소유 아이템의 `#[cfg]` —보내는 간선이 물려받는다.
    pub cfg: &'a Option<String>,
    /// 소유 아이템이 선언된 파일.
    pub file: &'a Path,
    /// 소유 아이템의 바이트 범위.
    pub range: &'a Range<usize>,
    /// 소유 아이템이 선언된 크레이트 — 같은 파일·범위가 여러 크레이트
    /// 문맥으로 로드될 때(공유 include!·`#[path]`) 진짜 소유자를 가른다.
    pub krate: &'a str,
    /// 소유 아이템이 선언된 모듈의 정규 경로 — 같은 파일을 같은
    /// 크레이트 안의 여러 모듈이 공유할 때(`#[path]`) 크레이트만으로는
    /// 문맥이 안 갈리므로 모듈까지 대조한다.
    pub module: &'a str,
}

impl Engine {
    /// `dir`의 cargo 워크스페이스를 의미 DB로 로드하고 정의 인덱스를 만든다.
    /// `sites`는 syn이 수확한 정규 ID → 선언 위치(파일·바이트 범위) —
    /// 생성 정의의 정점 provenance 검증에 쓴다.
    /// 빌드 스크립트는 `cargo check`로 한 번 실행한다(load_out_dirs_from_check) —
    /// `include!(concat!(env!("OUT_DIR"), ..))`와 proc 매크로 dylib이 여기서
    /// 준비된다. 산출물은 `target/rust-analyzer`에 모아 분석 대상의 target을
    /// 더럽히지 않는다. 로드 실패는 오류 — syn으로 조용히 떨어지면
    /// --semantic이 거짓말이 된다.
    pub fn load(dir: &Path, sites: &BTreeMap<String, Vec<Site>>) -> Result<Engine, String> {
        // sysroot 없이는 std 매크로(println! 류)조차 해석되지 않는다 —
        // 확장 실패는 인자 속 호출까지 통째로 잃으므로 반드시 켠다.
        let cargo_config = CargoConfig {
            sysroot: Some(RustLibSource::Discover),
            target_dir_config: TargetDirectoryConfig::UseSubdirectory,
            ..CargoConfig::default()
        };
        let load_config = LoadCargoConfig {
            load_out_dirs_from_check: true,
            with_proc_macro_server: ProcMacroServerChoice::Sysroot,
            prefill_caches: false,
            num_worker_threads: std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1),
            proc_macro_processes: 1,
        };
        let (db, vfs, proc_macro) = load_workspace_at(dir, &cargo_config, &load_config, &|_| {})
            .map_err(|e| format!("semantic engine could not load {}: {e:#}", dir.display()))?;
        // syn 선언 위치를 vfs 좌표로 변환해 둔다 — hir 정의의 원본
        // 파일·범위와 직접 비교해 정점 provenance를 확인한다.
        let mut site_map: BTreeMap<String, Vec<SynSite>> = BTreeMap::new();
        for (id, ss) in sites {
            for s in ss {
                let Some((file, _)) = vfs.file_id(&VfsPath::new_real_path(
                    s.file.to_string_lossy().into_owned(),
                )) else {
                    continue;
                };
                let (Ok(start), Ok(end)) =
                    (u32::try_from(s.range.start), u32::try_from(s.range.end))
                else {
                    continue;
                };
                site_map.entry(id.clone()).or_default().push(SynSite {
                    file,
                    range: TextRange::new(TextSize::from(start), TextSize::from(end)),
                    module: s.module.clone(),
                    def: None,
                });
            }
        }
        // 타입 소속(self_ty)·트레이트 쿼리는 next-solver의 스레드 로컬
        // attached db를 요구한다 — 인덱스 빌드·사이트 서명 해석부터 붙인다.
        let (sites, index) = ra_ap_hir::attach_db(&db, || {
            let sema = Semantics::new(&db);
            let mut site_map = site_map;
            for ss in site_map.values_mut() {
                for s in ss.iter_mut() {
                    s.def = site_def(&sema, s.file, s.range, &s.module);
                }
            }
            let index = build_index(&db, &site_map);
            (site_map, index)
        });
        Ok(Engine {
            db,
            vfs,
            proc_macro,
            defs: index.defs,
            sites,
            impls: index.impls,
            matrix: index.matrix,
        })
    }

    /// proc 매크로 서버가 붙었는가 — 없으면 proc 매크로 호출은 unexpanded로 간다.
    pub fn has_proc_macros(&self) -> bool {
        self.proc_macro.is_some()
    }

    /// 본문 하나를 의미 해석으로 걷는다 — 호출·참조·매크로 간선.
    /// `site`의 파일·범위는 syn이 본 소유 아이템의 소스 위치다 — 정규 ID가
    /// 같아도 소스 정체가 다르면(cfg 변형·블록 지역 정의) 다른 정의이므로
    /// None으로 되돌려 syn 폴백시킨다. 시그니처 간선은 항상 syn 경로다.
    pub fn body_edges(
        &self,
        site: &OwnerSite<'_>,
        ids: &BTreeSet<&str>,
        method_index: &BTreeMap<String, Vec<String>>,
        st: &mut Stats,
    ) -> Option<Vec<Edge>> {
        let entries = self.defs.get(site.id)?;
        let (file_id, _) = self.vfs.file_id(&VfsPath::new_real_path(
            site.file.to_string_lossy().into_owned(),
        ))?;
        let (Ok(start), Ok(end)) = (
            u32::try_from(site.range.start),
            u32::try_from(site.range.end),
        ) else {
            return None;
        };
        let want = TextRange::new(TextSize::from(start), TextSize::from(end));
        ra_ap_hir::attach_db(&self.db, || {
            let sema = Semantics::new(&self.db);
            // 사이트 선언이 가리키는 hir 정의 — def 정체가 곧 정체다.
            // syn이 못 잡는 외부 트레이트·별칭·섀도잉된 인자도 같은 def면
            // 같은 아이템이고, span을 재사용한 생성 정의는 다른 def라
            // 걸러진다.
            let site_def = site_def(&sema, file_id, want, site.module);
            // 같은 ID의 정의가 여럿이면(제네릭 인자가 다른 impl·cfg 변형)
            // 파일·이름 앵커로 진짜 소유자를 고른다 — intersect는 맞닿은
            // 범위도 성공시키므로 이름 토큰의 포함 여부로 대조한다.
            // 같은 소스 위치가 여러 크레이트 문맥으로 로드되거나(공유
            // include!·`#[path]`), span을 보존하는 생성 선언이 진짜 선언과
            // 겹칠 수 있다 — 파일·범위·크레이트에 def 정체까지 대조해
            // 정확히 하나의 항목만 고른다. 남는 게 없거나 여럿이면 잘못된
            // 본문의 간선이 이 정점에 귀속될 수 있으므로 억지로 고르지
            // 않고 syn 폴백으로 돌린다.
            let mut matched = entries.iter().filter(|e| {
                e.file == file_id
                    && want.contains_range(e.anchor.unwrap_or(e.range))
                    && e.krate == site.krate
                    && site_def == Some(e.def)
            });
            let entry = match (matched.next(), matched.next()) {
                (Some(e), None) => e,
                _ => return None,
            };
            let root = entry.def.body_root(&sema)?;
            let mut w = Walker {
                sema: &sema,
                owner: site.id,
                cfg: site.cfg,
                ids,
                method_index,
                sites: &self.sites,
                impls: &self.impls,
                matrix: &self.matrix,
                st,
                edges: Vec::new(),
            };
            w.walk(&root, false, 0);
            Some(w.edges)
        })
    }
}

/// 정규 ID → hir 정의 인덱스를 만든다 — 워크스페이스(Local) 크레이트만.
/// 정점은 syn이 만들었으므로 여기서는 "본문 소유자를 찾는" 매핑만 필요하다.
/// 모듈은 루트에서 children으로만 걷는다 — `krate.modules()`는 fn 안의
/// 블록 모듈까지 포함해 정규 경로가 부모 체인과 어긋날 수 있다.
fn build_index(db: &RootDatabase, sites: &BTreeMap<String, Vec<SynSite>>) -> Index {
    let sema = Semantics::new(db);
    let mut defs: BTreeMap<String, Vec<BodyEntry>> = BTreeMap::new();
    let mut impls: HashSet<Impl> = HashSet::new();
    let mut matrix: BTreeMap<(String, String), Vec<Candidate>> = BTreeMap::new();
    let mut stack: Vec<Module> = Vec::new();
    for krate in Crate::all(db) {
        if !krate.origin(db).is_local() {
            continue;
        }
        stack.push(krate.root_module(db));
        while let Some(module) = stack.pop() {
            for def in module.declarations(db) {
                match def {
                    ModuleDef::Function(f) => push_fn(&sema, &impls, &mut defs, f),
                    ModuleDef::Const(c) => {
                        if let Some(id) = const_id(db, c) {
                            if let Some(e) = body_entry(&sema, &impls, BodyDef::Const(c)) {
                                defs.entry(id).or_default().push(e);
                            }
                        }
                    }
                    ModuleDef::Static(s) => {
                        if let Some(id) = static_id(db, s) {
                            if let Some(e) = body_entry(&sema, &impls, BodyDef::Static(s)) {
                                defs.entry(id).or_default().push(e);
                            }
                        }
                    }
                    ModuleDef::Trait(t) => {
                        // 트레이트 기본 메서드 — 선언점 메서드 정점의 본문.
                        for item in t.items(db) {
                            if let AssocItem::Function(f) = item {
                                push_fn(&sema, &impls, &mut defs, f);
                            }
                        }
                    }
                    _ => {}
                }
            }
            stack.extend(module.children(db));
        }
        // impl 블록 메서드 — 선언 모듈이 아니라 self 타입 소속으로 ID를 만든다.
        // all_in_crate는 익명 const 블록(serde_derive 패턴)까지 따라가지만
        // fn 본문 안 블록 모듈의 impl은 닿지 않는다 — 이 집합이 "크레이트
        // 스코프 impl"의 정의가 된다.
        for imp in Impl::all_in_crate(db, krate) {
            impls.insert(imp);
            // self 타입이 블록 지역인 impl — 정규 ID가 같은 이름의 모듈
            // 정점과 충돌할 수 있다. impl 구문이 const 래퍼·매크로 확장
            // 안에 있어도 self 타입이 모듈 레벨이면 후보는 유효하다.
            // 반대로 `const _:()={ struct S; impl Tr for S }`처럼 확장이
            // 만든 지역 타입의 impl은 모듈 정점을 훔치면 안 된다.
            if imp
                .self_ty(db)
                .as_adt()
                .is_some_and(|a| adt_block_local(&sema, a))
            {
                continue;
            }
            // 트레이트 impl이면 디스패치 후보 표에도 넣는다.
            let tid = imp.trait_(db).map(|t| trait_id(db, t));
            // derive·매크로가 만든 impl은 메서드 정점이 없을 수 있다 —
            // 그때는 impl 대상 타입 정점으로 폴백한다.
            let owner = if impl_is_generated(&sema, imp) {
                imp.self_ty(db).as_adt().map(|a| adt_id(db, a))
            } else {
                None
            };
            for item in imp.items(db) {
                let AssocItem::Function(f) = item else {
                    continue;
                };
                push_fn(&sema, &impls, &mut defs, f);
                if let Some(tid) = &tid {
                    // 후보 ID는 syn provenance가 확인될 때만 — fn_id는
                    // 문자열이라 생성 메서드가 같은 이름의 진짜 정점과
                    // 충돌할 수 있다. 메서드·타깃 둘 다 표현 불가인 후보
                    // (blanket·원시 타입 impl)도 버리지 않고 남긴다 —
                    // 디스패치 지점에서 external로 세어져야 limitation이
                    // 정직하다.
                    let method = fn_id(&sema, f).filter(|id| syn_backed(sites, f, id));
                    matrix
                        .entry((tid.clone(), f.name(db).as_str().to_string()))
                        .or_default()
                        .push(Candidate {
                            method,
                            owner: owner.clone(),
                        });
                }
            }
        }
    }
    Index {
        defs,
        impls,
        matrix,
    }
}

impl BodyDef {
    /// 정의가 선언된 모듈 — 사이트의 기대 모듈과 대조해 공유 파일의
    /// 문맥 혼동을 걸러낸다.
    fn module(&self, db: &dyn HirDatabase) -> Module {
        match self {
            BodyDef::Fn(f) => f.module(db),
            BodyDef::Const(c) => c.module(db),
            BodyDef::Static(s) => s.module(db),
        }
    }

    /// 정의의 본문 루트 — 아이템이 아니라 본문(또는 초기값)부터 걷는다.
    /// 블록 안에 선언된 중첩 정의의 본문이 소유자에게 귀속되지 않도록,
    /// 그리고 시그니처 표면이 본문 간선으로 새지 않도록 하는 시작점이다.
    fn body_root(&self, sema: &Semantics<RootDatabase>) -> Option<SyntaxNode> {
        match self {
            BodyDef::Fn(f) => Some(sema.source(*f)?.value.body()?.syntax().clone()),
            BodyDef::Const(c) => Some(sema.source(*c)?.value.body()?.syntax().clone()),
            BodyDef::Static(s) => Some(sema.source(*s)?.value.body()?.syntax().clone()),
        }
    }
}

/// 정규 ID가 syn이 실제 수확한 선언을 가리키는가 — fn_id는 이름만 보는
/// 문자열이라 생성 메서드가 같은 이름의 진짜 정점 ID와 충돌할 수 있다.
/// 사이트 위치의 선언을 ra가 해석한 def가 이 메서드의 def와 같아야
/// 같은 정의다 — intern ID 비교라 `a::Tr`/`b::Tr`처럼 같은 정규 ID를
/// 갖는 다른 impl의 메서드는 구분되고, 인덱스 자기 항목과의 비교처럼
/// 생성 정의가 자기 자신으로 검증을 통과하는 구멍도 없다.
fn syn_backed(sites: &BTreeMap<String, Vec<SynSite>>, f: Function, id: &str) -> bool {
    sites
        .get(id)
        .is_some_and(|ss| ss.iter().any(|s| s.def == Some(BodyDef::Fn(f))))
}

/// 사이트 선언이 가리키는 hir 정의 — syn이 수확한 위치에서 선언을
/// 찾아 def로 해석한다. def는 intern된 ID라 정체 비교가 곧 같은
/// 아이템 판정이다 — 문자열을 거치지 않으므로 이름 해석·렌더링의
/// 손실이 개입할 틈이 없다. 속성 매크로 입력 토큰은 아이템 트리에
/// 없으므로 확장을 따라가 사본의 def를 읽는다. 해석 불가·애매하면
/// None — 호출자가 syn 폴백으로 돌린다.
fn site_def(
    sema: &Semantics<RootDatabase>,
    file: FileId,
    range: TextRange,
    want_module: &str,
) -> Option<BodyDef> {
    let parsed = sema.parse(sema.attach_first_edition_opt(file)?);
    // preorder라 바깥 아이템이 먼저 온다 — 이름 토큰이 사이트 범위 안에
    // 있는 가장 바깥 선언이 사이트 아이템이다. `const C: () = { impl .. {
    // fn m } }`처럼 안쪽 선언도 이름은 범위 안에 있지만 바깥 소유자가
    // 먼저 잡힌다.
    let anchored =
        |n: Option<ast::Name>| n.is_some_and(|nm| range.contains_range(nm.syntax().text_range()));
    let def = parsed.syntax().descendants().find_map(|n| {
        if let Some(f) = ast::Fn::cast(n.clone()) {
            if !anchored(f.name()) {
                return None;
            }
            return Some(site_fn_def(sema, f, file, range));
        }
        if let Some(c) = ast::Const::cast(n.clone()) {
            if !anchored(c.name()) {
                return None;
            }
            return Some(sema.to_def(&c).map(BodyDef::Const));
        }
        if let Some(s) = ast::Static::cast(n) {
            if !anchored(s.name()) {
                return None;
            }
            return Some(sema.to_def(&s).map(BodyDef::Static));
        }
        None
    })??;
    // 같은 물리 파일을 여러 모듈이 가리키면(`#[path]`·공유 파일) ra는
    // 임의의 하나의 문맥으로 def를 묶을 수 있다 — 소유 모듈이 syn이
    // 수확한 모듈과 다르면 이 문맥의 선언이 아니므로 버린다.
    (module_path(sema.db, def.module(sema.db)) == want_module).then_some(def)
}

/// fn 사이트의 hir 정의 — 아이템 트리에 있으면 그 def가 곧 정체다.
/// 속성 매크로 입력 토큰은 아이템 트리에 없으므로 확장 안의 사본을
/// 찾는다: impl 멤버면 헤더 토큰까지 입력 것인 사본만 믿고, 자유
/// fn이면 이름 앵커로 충분하다. 사본이 여러 def로 갈리면 어느 것이
/// 사이트의 것인지 모르므로 None이다.
fn site_fn_def(
    sema: &Semantics<RootDatabase>,
    f: ast::Fn,
    file: FileId,
    site: TextRange,
) -> Option<BodyDef> {
    if let Some(d) = sema.to_fn_def(&f) {
        return Some(BodyDef::Fn(d));
    }
    let mut defs = HashSet::new();
    match f
        .syntax()
        .parent()
        .and_then(|p| p.parent())
        .and_then(ast::Impl::cast)
    {
        Some(imp) => {
            // 입력 헤더의 원본 범위 — 사본 검증 키. 레벨 0은 실제
            // 파일이므로 구문 범위가 곧 원본 범위다.
            let header = (
                imp.trait_().map(|t| t.syntax().text_range()),
                imp.self_ty().map(|t| t.syntax().text_range()),
            );
            expanded_fn_defs(
                sema,
                ast::Item::Impl(imp),
                file,
                site,
                Some(header),
                0,
                &mut defs,
            )?;
        }
        None => {
            expanded_fn_defs(sema, ast::Item::Fn(f), file, site, None, 0, &mut defs)?;
        }
    }
    if defs.len() == 1 {
        defs.into_iter().next().map(BodyDef::Fn)
    } else {
        None
    }
}

/// 속성 매크로 확장 안에서 사이트 선언의 사본 fn들의 def를 모은다.
/// 사본 판정은 두 층이다 — (1) 이름 토큰의 원본 범위가 사이트 안이어야
/// 한다(메서드 전체를 요구하면 사본에 붙은 새 속성이 원본 범위를
/// 키워 진짜 사본을 걸러낸다), (2) impl 멤버 입력이면 사본을 담은
/// impl의 트레이트·self 타입 토큰 원본 범위가 입력 헤더와 같아야
/// 한다 — call-site span의 형제 impl은 여기서 걸러진다.
/// 확장이 `m! { .. }`처럼 함수형 매크로 호출을 내면 그 안도 재귀적으로
/// 본다 — 토큰 트리 안에 경쟁 사본이 숨을 수 있으므로 확장하지 못하는
/// 아이템 위치 호출이 있으면 후보 집합의 완전성을 증명 못 해 애매로
/// 본다(None). fn 본문 안의 호출은 블록 지역 아이템만 만들 수 있어
/// 건너뛴다.
/// 사본이 다시 매크로 입력이면 확장을 반복한다. 어느 후보든 정체를
/// 확립하지 못하면(None 전파) 결과는 애매다.
fn expanded_fn_defs(
    sema: &Semantics<RootDatabase>,
    item: ast::Item,
    file: FileId,
    site: TextRange,
    header: Option<(Option<TextRange>, Option<TextRange>)>,
    depth: usize,
    defs: &mut HashSet<Function>,
) -> Option<()> {
    if depth >= MAX_EXPANSION_DEPTH {
        return None;
    }
    let er = sema.expand_attr_macro(&item)?;
    // 확장은 성공해도 내부 확장 에러가 있으면 트리가 불완전하다 —
    // 후보 집합이 불완전한 채로 단독 후보를 채택하면 안 되므로 애매로 본다.
    if er.err.is_some() {
        return None;
    }
    walk_expansion(sema, &er.value, file, site, header, depth, defs)
}

/// 확장 트리를 걸어 사본 후보를 모은다 — `expanded_fn_defs`의 재귀
/// 본체. 세 종류의 숨은 후보원을 함께 연다 — (1) 아이템 위치의 함수형
/// 매크로 호출(토큰 트리라 descendants로 안이 안 보임), (2) 아이템에
/// 달린 속성 매크로(인자 토큰으로 사본을 emit할 수 있음), (3) 사본이
/// 다시 매크로 입력인 경우. 어느 것이든 확장에 실패하거나 내부 확장
/// 에러가 있으면 후보 완전성을 증명 못 해 애매로 본다(None). fn 본문
/// 안의 호출·아이템은 블록 지역이라 정점 소유권과 무관해 건너뛴다.
fn walk_expansion(
    sema: &Semantics<RootDatabase>,
    root: &InFile<SyntaxNode>,
    file: FileId,
    site: TextRange,
    header: Option<(Option<TextRange>, Option<TextRange>)>,
    depth: usize,
    defs: &mut HashSet<Function>,
) -> Option<()> {
    if depth >= MAX_EXPANSION_DEPTH {
        return None;
    }
    // 확장 노드의 원본 범위 — 사이트와 같은 실제 파일 좌표다.
    let orig_range = |n: &SyntaxNode| {
        sema.original_range_opt(n)
            .and_then(|fr| (fr.file_id.file_id(sema.db) == file).then_some(fr.range))
    };
    let mut pending: Vec<ast::Item> = Vec::new();
    for n in root.value.descendants() {
        if let Some(mc) = ast::MacroCall::cast(n.clone()) {
            // fn 본문 안 호출은 블록 지역 아이템만 만들 수 있다 — 정점
            // 소유권과 무관하므로 건너뛴다. 아이템 위치 호출은 안쪽이
            // 보이지 않으면 후보 완전성을 증명 못 해 애매로 본다.
            if n.ancestors().any(|a| ast::Fn::cast(a).is_some()) {
                continue;
            }
            // parse_or_expand 계열은 확장 에러를 삼켜 빈 트리를 줄 수
            // 있으므로 MacroCallId 경로로 err까지 확인한다.
            let call_id: ra_ap_hir::MacroCallId = sema.to_def(&mc)?;
            let er = sema.expand(call_id);
            if er.err.is_some() {
                return None;
            }
            walk_expansion(
                sema,
                &InFile::new(call_id.into(), er.value),
                file,
                site,
                header,
                depth + 1,
                defs,
            )?;
            continue;
        }
        // 아이템 위치의 속성 매크로 — 인자 토큰이 사본을 숨길 수 있어
        // fn이 보이든 안 보이든 함께 확장한다. `is_attr_macro_call`은
        // derive·내장 속성을 걸러낸다.
        if let Some(it) = ast::Item::cast(n.clone()) {
            if it.attrs().next().is_some()
                && !n.ancestors().any(|a| ast::Fn::cast(a).is_some())
                && sema.is_attr_macro_call(InFile::new(root.file_id, &it))
            {
                pending.push(it);
                continue;
            }
        }
        let Some(ef) = ast::Fn::cast(n) else {
            continue;
        };
        let anchored = ef
            .name()
            .and_then(|n| orig_range(n.syntax()))
            .is_some_and(|r| site.contains_range(r));
        if !anchored {
            continue;
        }
        // 소속 impl — 함수형 매크로 안에 들어간 사본은 조상이 별도
        // 확장 트리에 있으므로 확장을 건너는 조상 순회로 찾는다.
        let eimp = sema
            .ancestors_with_macros(ef.syntax().clone())
            .find_map(ast::Impl::cast);
        // impl 멤버 입력에서는 헤더가 입력 토큰 사본인 impl만 믿는다 —
        // 헤더가 다른 형제 impl의 이름 재사용 메서드는 후보가 아니다.
        // impl 밖 사본(입력은 impl 멤버인데 확장에서 자유 fn)과 자유 fn
        // 입력의 impl 안 사본은 정체가 달라질 수 있어 후보로 남겨
        // 애매를 유도한다.
        if let (Some((want_trait, want_self)), Some(ei)) = (header, &eimp) {
            let same_header = ei.trait_().and_then(|t| orig_range(t.syntax())) == want_trait
                && ei.self_ty().and_then(|t| orig_range(t.syntax())) == want_self;
            if !same_header {
                continue;
            }
        }
        match sema.to_fn_def(&ef) {
            Some(d) => {
                defs.insert(d);
            }
            None => {
                // 사본이 다시 매크로 입력 — impl에 매크로가 달려 있으면
                // impl을, 아니면 메서드 자체의 매크로를 다음 단계에서
                // 확장한다. 확장 결과를 바로 재귀로 걷는다(단방향 재귀 —
                // 호출 사이클을 만들지 않는다).
                let next = match &eimp {
                    Some(ei) if ei.attrs().next().is_some() => ast::Item::Impl(ei.clone()),
                    _ => ast::Item::Fn(ef),
                };
                pending.push(next);
            }
        }
    }
    for p in pending {
        // 재귀 깊이 상한은 walk_expansion 입구에서 걸린다.
        let er = sema.expand_attr_macro(&p)?;
        if er.err.is_some() {
            return None;
        }
        walk_expansion(sema, &er.value, file, site, header, depth + 1, defs)?;
    }
    Some(())
}

/// 정의의 소스 정체를 잡아 인덱스 항목으로 만든다 — 블록(fn 본문) 안에
/// 선언된 정의와 매크로가 만든 정의(원본 파일 범위가 없는 것)는 인덱스하지
/// 않는다: 둘 다 정규 ID가 실제 정점과 충돌할 수 있기 때문이다.
/// 지역성은 세 층으로 판정한다 — (1) impl 메서드는 소속 impl이
/// `all_in_crate`에 있어야 한다(fn 본문 안 모듈의 impl은 없고 익명
/// const 안의 impl은 있다), (2) 그 외 정의는 소속 모듈이 블록 모듈이면
/// 안 된다(`#[path]`·include!로 같은 파일을 가리키는 지역 모듈은
/// 구문 조상만으로는 구분이 안 된다), (3) 실제 파일 조상에 BlockExpr가
/// 있으면 fn 안 선언이다. 확장 파일 내부의 블록(익명 const 래퍼)은
/// 지역으로 치지 않는다.
fn body_entry(
    sema: &Semantics<RootDatabase>,
    impls: &HashSet<Impl>,
    def: BodyDef,
) -> Option<BodyEntry> {
    let (node, name_node, module, in_scope) = match def {
        BodyDef::Fn(f) => {
            let s = sema.source(f)?;
            let in_scope = match f.as_assoc_item(sema.db).map(|a| a.container(sema.db)) {
                Some(AssocItemContainer::Impl(i)) => impls.contains(&i),
                _ => !module_is_local(sema.db, f.module(sema.db)),
            };
            (
                InFile::new(s.file_id, s.value.syntax().clone()),
                s.value.name(),
                f.module(sema.db),
                in_scope,
            )
        }
        BodyDef::Const(c) => {
            let s = sema.source(c)?;
            let m = c.module(sema.db);
            (
                InFile::new(s.file_id, s.value.syntax().clone()),
                s.value.name(),
                m,
                !module_is_local(sema.db, m),
            )
        }
        BodyDef::Static(s) => {
            let src = sema.source(s)?;
            let m = s.module(sema.db);
            (
                InFile::new(src.file_id, src.value.syntax().clone()),
                src.value.name(),
                m,
                !module_is_local(sema.db, m),
            )
        }
    };
    if !in_scope {
        return None;
    }
    // 확장 파일 안의 블록(익명 const 래퍼)은 지역으로 치지 않는다 —
    // fn 본문 안 선언은 실제 파일 조상에 BlockExpr가 있어야 한다.
    if sema
        .ancestors_with_macros_file(node.clone())
        .any(|a| !a.file_id.is_macro() && ast::BlockExpr::cast(a.value).is_some())
    {
        return None;
    }
    let fr = sema.original_range_opt(&node.value)?;
    let anchor = name_node
        .and_then(|n| sema.original_range_opt(n.syntax()))
        .map(|a| a.range);
    Some(BodyEntry {
        def,
        krate: crate_name(sema.db, module.krate(sema.db)),
        file: fr.file_id.file_id(sema.db),
        range: fr.range,
        anchor,
    })
}

/// impl 블록이 코드 생성물인가 — `#[derive]`가 만드는 builtin impl은 소스가
/// 없고, proc 매크로·macro_rules 확장 안의 impl은 매크로 파일 소스다.
/// 둘 다 syn이 만든 정점이 없으므로 호출은 impl 대상 타입으로 귀속한다.
fn impl_is_generated(sema: &Semantics<RootDatabase>, i: Impl) -> bool {
    match sema.source(i) {
        None => true,
        Some(s) => s.file_id.is_macro(),
    }
}

/// 모듈이 블록 스코프 안에 사는가 — 자신 또는 어느 조상이든 블록
/// 모듈이면 지역이다. fn 안의 *named* 모듈(`fn f() { mod shared; }`)은
/// 자기 자신은 블록이 아니지만 부모가 블록이므로 조상까지 봐야 한다 —
/// 이 경우 아이템의 정규 ID가 모듈 선언과 정확히 겹친다.
fn module_is_local(db: &dyn HirDatabase, m: Module) -> bool {
    m.path_to_root(db)
        .into_iter()
        .any(|a| a != a.nearest_non_block_module(db))
}

/// ADT가 블록 스코프에 사는가 — 직접 fn 안에 선언됐거나 소속 모듈이
/// fn 안 `#[path] mod` 같은 블록 스코프면 지역이다. 후자는 같은 원본
/// 파일을 공유하는 지역 모듈의 타입이 모듈 정점을 훔치는 것을 막는다.
fn adt_block_local(sema: &Semantics<RootDatabase>, a: Adt) -> bool {
    block_local_def(sema, a) || module_is_local(sema.db, a.module(sema.db))
}

/// hir 정의가 fn 본문 안의 지역 선언인가 — 소스를 얻을 수 없는 정의는
/// false. 조상은 확장 인지로 걷되 *실제 파일*의 BlockExpr만 본다 —
/// 매크로가 함수 안에서 만든 타입은 호출 지점까지 올라가야 fn이 보이고,
/// 반대로 `const _: () = { impl .. }`(serde_derive 패턴)처럼 확장이
/// 만든 블록 래퍼는 조상에 있어도 지역이 아니다.
fn block_local_def<D: ra_ap_hir::HasSource>(sema: &Semantics<RootDatabase>, d: D) -> bool
where
    D::Ast: AstNode,
{
    sema.source(d).is_some_and(|s| {
        let node = InFile::new(s.file_id, s.value.syntax().clone());
        sema.ancestors_with_macros_file(node)
            .any(|a| !a.file_id.is_macro() && ast::BlockExpr::cast(a.value).is_some())
    })
}

/// 함수 한 개를 ID 계산해 entries에 넣는다 — 클로저로 쓰면 entries와
/// db 빌림이 얽혀서 자유 함수다.
fn push_fn(
    sema: &Semantics<RootDatabase>,
    impls: &HashSet<Impl>,
    defs: &mut BTreeMap<String, Vec<BodyEntry>>,
    f: Function,
) {
    if let Some(id) = fn_id(sema, f) {
        if let Some(e) = body_entry(sema, impls, BodyDef::Fn(f)) {
            // 같은 ID의 정의가 여럿일 수 있다(`impl S<u8>`/`S<u16>`,
            // cfg 변형) — 덮어쓰지 않고 전부 보관한다.
            defs.entry(id).or_default().push(e);
        }
    }
}

/// 모듈의 정규 경로 — `crate` 또는 `crate::a::b`. 루트 모듈은 이름이 없어
/// 크레이트 이름을 그대로 쓴다(syn 경로의 루트 정점과 같은 ID).
fn module_path(db: &dyn HirDatabase, module: Module) -> String {
    let mut segs = Vec::new();
    let mut cur = module;
    while let Some(parent) = cur.parent(db) {
        if let Some(name) = cur.name(db) {
            segs.push(name.as_str().to_string());
        }
        cur = parent;
    }
    let krate = crate_name(db, cur.krate(db));
    if segs.is_empty() {
        krate
    } else {
        segs.reverse();
        format!("{krate}::{}", segs.join("::"))
    }
}

/// 크레이트 표시 이름을 정규 ID용으로 — cargo 이름의 `-`는 `_`로.
fn crate_name(db: &dyn HirDatabase, krate: Crate) -> String {
    krate
        .display_name(db)
        .map(|d| normalize_name(d.canonical_name().as_str()))
        .unwrap_or_else(|| "?".to_string())
}

/// ADT의 정규 ID — `crate::mod::Type`.
fn adt_id(db: &dyn HirDatabase, adt: Adt) -> String {
    format!(
        "{}::{}",
        module_path(db, adt.module(db)),
        adt.name(db).as_str()
    )
}

/// 함수 정규 ID — 자유 함수 `m::f`, 고유 메서드 `T::m`, 트레이트 impl
/// 메서드 `T::<Tr>::m`, 트레이트 선언 메서드 `m::Tr::m`. syn 수확과 같은
/// 스킴이다 — 같은 코드가 같은 ID를 가져야 간선이 만난다.
fn fn_id(sema: &Semantics<RootDatabase>, f: Function) -> Option<String> {
    let db = sema.db;
    let name = f.name(db).as_str().to_string();
    if let Some(assoc) = f.as_assoc_item(db) {
        match assoc.container(db) {
            AssocItemContainer::Trait(t) => {
                return Some(format!(
                    "{}::{}::{name}",
                    module_path(db, t.module(db)),
                    t.name(db).as_str()
                ));
            }
            AssocItemContainer::Impl(i) => {
                let ty = i.self_ty(db).as_adt()?;
                let base = adt_id(db, ty);
                return Some(match i.trait_(db) {
                    // syn 정점은 해석된 이름이 아니라 impl에 쓰인 이름을 쓴다 —
                    // `use Tr as Alias` 뒤 `impl Alias for T`면 정점은
                    // `T::<Alias>::m`이다. 쓰인 세그먼트를 우선한다.
                    Some(t) => {
                        let tname = written_trait_name(sema, i)
                            .unwrap_or_else(|| t.name(db).as_str().to_string());
                        format!("{base}::<{tname}>::{name}")
                    }
                    None => format!("{base}::{name}"),
                });
            }
        }
    }
    Some(format!("{}::{name}", module_path(db, f.module(db))))
}

/// impl 구문에 쓰인 트레이트 경로의 마지막 세그먼트 — 별칭이면 hir의
/// 정식 이름 대신 이것이 syn 정점 ID의 트레이트 부분과 일치한다.
fn written_trait_name(sema: &Semantics<RootDatabase>, i: Impl) -> Option<String> {
    let ty = sema.source(i)?.value.trait_()?;
    let ast::Type::PathType(pt) = ty else {
        return None;
    };
    let seg = pt.path()?.segment()?;
    Some(seg.name_ref()?.ident_token()?.text().to_string())
}

/// 상수 정규 ID — 모듈 소속 상수만. 연관 상수(`impl T { const N }`)는
/// syn이 `m::N` 정점을 만들지 않으므로 정규 ID로 표현할 수 없다 —
/// 여기서 None을 주면 호출자가 그래프 밖(external)으로 센다.
fn const_id(db: &dyn HirDatabase, c: Const) -> Option<String> {
    if c.as_assoc_item(db).is_some() {
        return None;
    }
    Some(format!(
        "{}::{}",
        module_path(db, c.module(db)),
        c.name(db)?.as_str()
    ))
}

fn static_id(db: &dyn HirDatabase, s: Static) -> Option<String> {
    Some(format!(
        "{}::{}",
        module_path(db, s.module(db)),
        s.name(db).as_str()
    ))
}

fn trait_id(db: &dyn HirDatabase, t: Trait) -> String {
    format!("{}::{}", module_path(db, t.module(db)), t.name(db).as_str())
}

fn alias_id(db: &dyn HirDatabase, t: TypeAlias) -> String {
    format!("{}::{}", module_path(db, t.module(db)), t.name(db).as_str())
}

fn enum_id(db: &dyn HirDatabase, e: Enum) -> String {
    format!("{}::{}", module_path(db, e.module(db)), e.name(db).as_str())
}

fn macro_id(db: &dyn HirDatabase, m: Macro) -> String {
    format!("{}::{}", module_path(db, m.module(db)), m.name(db).as_str())
}

/// push의 결과 — 호출자가 실패 종류별로 다른 버킷에 센다.
enum Pushed {
    Yes,
    /// 자기 자신에게로 가는 간선 — 조용히 버린다(syn과 같은 규칙).
    SelfEdge,
    /// 해석은 됐지만 그래프에 정점이 없다 — 외부·생성 정의.
    Miss,
}

/// 본문 방문자 — ra 구문 트리를 걸으며 호출·참조를 간선으로 옮긴다.
/// `unsafe {}` 블록 안의 간선은 경계 진입으로 표시한다(syn 계약과 동일).
struct Walker<'a, 'b> {
    sema: &'a Semantics<'a, RootDatabase>,
    owner: &'a str,
    cfg: &'a Option<String>,
    ids: &'a BTreeSet<&'a str>,
    method_index: &'a BTreeMap<String, Vec<String>>,
    /// 정규 ID → syn이 수확한 선언 위치 — 생성·충돌 정의를 가른다.
    sites: &'a BTreeMap<String, Vec<SynSite>>,
    /// 크레이트 스코프 impl 집합 — fn 블록 모듈의 impl 구분에 쓴다.
    impls: &'a HashSet<Impl>,
    /// (트레이트 ID, 메서드 이름) → impl 메서드 후보 — 빌드 때 계산됐다.
    matrix: &'a BTreeMap<(String, String), Vec<Candidate>>,
    st: &'b mut Stats,
    edges: Vec<Edge>,
}

impl<'a, 'b> Walker<'a, 'b> {
    /// DB 참조 — 'a 생명주기로 꺼내므로 self를 빌리지 않는다.
    /// 이걸 &self 생명주기로 주면 push 같은 &mut self와 겹쳐 빌드가 안 된다.
    fn db(&self) -> &'a dyn HirDatabase {
        self.sema.db
    }

    /// 간선 하나. tentative면 추정(디스패치 후보·이름 팬아웃), 아니면 확정.
    fn push(&mut self, to: String, kind: EdgeKind, tentative: bool, in_unsafe: bool) -> Pushed {
        if to == self.owner {
            return Pushed::SelfEdge;
        }
        if !self.ids.contains(to.as_str()) {
            return Pushed::Miss;
        }
        let mut e = Edge::new(self.owner.to_string(), to, kind);
        e.tentative = tentative;
        e.cfg = self.cfg.clone();
        e.unsafe_ = in_unsafe;
        self.edges.push(e);
        if !tentative {
            self.st.resolved += 1;
        }
        Pushed::Yes
    }

    /// 구문 트리 순회 — preorder, unsafe 블록 깊이를 따라간다.
    /// 매크로 호출은 확장 트리로도 내려간다(depth는 확장 재귀만 센다).
    /// 본문 안의 `아이템`(중첩 fn·impl·static 등)은 어떤 깊이에서도 들어가지
    /// 않는다 — 그 본문의 호출은 이 소유자의 것이 아니다. 다만 MacroCall은
    /// 아이템이면서 확장의 입구라 통과시킨다 — 중첩 매크로의 내부 호출을
    /// 수확하려면 확장으로 내려가는 길을 막으면 안 된다.
    fn walk(&mut self, node: &SyntaxNode, in_unsafe: bool, depth: usize) {
        if ast::Item::cast(node.clone()).is_some_and(|i| !matches!(i, ast::Item::MacroCall(_))) {
            return;
        }
        let un = in_unsafe
            || ast::BlockExpr::cast(node.clone()).is_some_and(|b| b.unsafe_token().is_some());
        if let Some(mc) = ast::MethodCallExpr::cast(node.clone()) {
            self.method_call(&mc, un);
        } else if let Some(call) = ast::CallExpr::cast(node.clone()) {
            self.call(&call, un);
        } else if let Some(re) = ast::RecordExpr::cast(node.clone()) {
            self.record(&re, un);
        } else if let Some(pe) = ast::PathExpr::cast(node.clone()) {
            self.path_ref(&pe, un);
        } else if let Some(p) = ast::PathPat::cast(node.clone()) {
            // `match` 패턴의 경로 — 상수·variant·타입 참조다.
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(p) = ast::RecordPat::cast(node.clone()) {
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(p) = ast::TupleStructPat::cast(node.clone()) {
            if let Some(path) = p.path() {
                self.emit_path(path, EdgeKind::References, un);
            }
        } else if let Some(mac) = ast::MacroCall::cast(node.clone()) {
            self.macro_call(&mac, un, depth);
        }
        for child in node.children() {
            self.walk(&child, un, depth);
        }
    }

    /// `x.m()` — 수신자 타입으로 정확히 해석한다. 해석되면 확정 간선,
    /// 트레이트 정의 메서드면 impl 행렬로, 끝까지 안 되면 이름 팬아웃.
    fn method_call(&mut self, mc: &ast::MethodCallExpr, un: bool) {
        if let Some(f) = self.sema.resolve_method_call(mc) {
            self.emit_function(f, EdgeKind::Call, un, self.receiver_is_concrete(mc));
            return;
        }
        // resolve_method_call_fallback은 dyn/제네릭에서도 트레이트 정의나
        // 필드를 돌려준다 — 필드면 호출이 아니니 버린다.
        // `m::<T>(..)`의 제네릭 인자까지 텍스트로 잡지 않게 토큰만 쓴다.
        let name = mc
            .name_ref()
            .and_then(|n| n.ident_token())
            .map(|t| t.text().to_string());
        match self.sema.resolve_method_call_fallback(mc) {
            // 필드 접근(left()==None)은 호출이 아니다 — 컴파일되지도 않는
            // 코드라 간선 없이 미해석으로만 센다.
            Some((res, _)) => {
                if let Some(f) = res.left() {
                    self.emit_function(f, EdgeKind::Call, un, self.receiver_is_concrete(mc));
                } else {
                    self.st.unresolved += 1;
                }
            }
            None => self.fan_out(name.as_deref(), un),
        }
    }

    /// 수신자 타입이 완전히 구체적인가 — dyn/impl Trait/제네릭이면
    /// 디스패치가 열려 있으므로 false다. `Box<dyn Tr>` 같은 스마트 포인터는
    /// 조정 전 타입이 ADT이므로 역참조·자동 참조가 반영된 조정 후 타입을 본다.
    fn receiver_is_concrete(&self, mc: &ast::MethodCallExpr) -> bool {
        mc.receiver()
            .and_then(|r| self.sema.type_of_expr(&r))
            .is_some_and(|t| self.type_is_concrete(&t.adjusted.unwrap_or(t.original)))
    }

    /// 타입이 완전히 구체적인가 — dyn·타입 파라미터·연관 타입·opaque·
    /// 미해석(infer 실패) 종류가 하나라도 섞이면 디스패치는 열려 있다.
    /// 겉이 ADT(Box 등)여도 인자 안에 그런 종류가 있으면 열린다 —
    /// `self: Box<Self>` 메서드는 조정 후에도 `Box<dyn Tr>` 형태가
    /// 유지되므로 겉 타입이 아니라 구성 타입 전부를 walk로 본다.
    /// 열린 종류를 나열하지 않고 구체로 확인된 종류만 허용한다 —
    /// `dyn Send`처럼 principal trait이 없어 `as_dyn_trait`가 못 잡는
    /// 객체나 미지의 kind도 안전한 쪽(열림)으로 떨어진다.
    fn type_is_concrete(&self, ty: &Type) -> bool {
        let mut concrete = true;
        ty.walk(self.db(), |t| {
            concrete &= t.as_adt().is_some()
                || t.as_builtin().is_some()
                || t.is_never()
                || t.is_tuple()
                || t.is_slice()
                || t.is_array()
                || t.as_reference().is_some()
                || t.is_raw_ptr()
                || t.is_fn()
                || t.is_closure()
                || t.as_coroutine().is_some();
        });
        concrete
    }

    /// 해석된 함수를 간선으로 — 트레이트 정의 메서드는, 수신자가 구체
    /// 타입으로 확인됐으면(기본 구현을 그대로 상속) 정의점이 확정 타깃이고,
    /// 디스패치가 열려 있으면(dyn·제네릭) impl 행렬로 펼친다.
    fn emit_function(&mut self, f: Function, kind: EdgeKind, un: bool, concrete: bool) {
        let db = self.db();
        // 지역성을 provenance보다 먼저 가린다 — `mod m { include!("x.rs") }`
        // 나 fn 안 `#[path]` 모듈 같은 지역 복제본은 원본 파일·범위가 모듈
        // 선언과 같아 syn_backed를 통과하므로, 범위 검사만으로는 모듈
        // 정점을 훔칠 수 있다.
        // impl 메서드는 all_in_crate 멤버십으로 판정한다 — fn 본문 안
        // 블록의 impl은 거기 없고, 익명 const 래퍼(serde_derive 패턴)의
        // impl은 있다. 그 외 정의는 소속 모듈이 블록 모듈이면 지역이다 —
        // fn 안 `#[path] mod`의 아이템은 원본 파일 조상에 BlockExpr가
        // 없어 구문 검사만으로는 못 잡는다.
        let local = match f.as_assoc_item(db).map(|a| a.container(db)) {
            Some(AssocItemContainer::Impl(i)) => {
                !self.impls.contains(&i)
                    || i.self_ty(db)
                        .as_adt()
                        .is_some_and(|a| adt_block_local(self.sema, a))
            }
            _ => block_local_def(self.sema, f) || module_is_local(db, f.module(db)),
        };
        if local {
            self.st.external += 1;
            return;
        }
        if let Some(t) = f.as_assoc_item(db).and_then(|a| a.container_trait(db)) {
            if concrete {
                return self.emit_fn_vertex(f, kind, un);
            }
            return self.trait_matrix(t, f, kind, un);
        }
        // 정규 ID가 인덱스의 실제 선언과 일치할 때만 정점으로 — fn_id는
        // 문자열이라 생성 메서드가 같은 이름의 진짜 정점과 충돌할 수 있다.
        // 속성 매크로가 감싼 impl의 메서드는 소스 정체가 일치한다.
        if let Some(id) = fn_id(self.sema, f) {
            if syn_backed(self.sites, f, &id)
                && matches!(
                    self.push(id, kind, false, un),
                    Pushed::Yes | Pushed::SelfEdge
                )
            {
                return;
            }
        }
        // 정점이 없는 정의 — derive·매크로가 만든 impl 메서드면 impl 대상
        // 타입으로 귀속한다(const 래퍼 안의 생성 impl 포함).
        if let Some(AssocItemContainer::Impl(i)) = f.as_assoc_item(db).map(|a| a.container(db)) {
            if impl_is_generated(self.sema, i) {
                return self.emit_impl_owner(i, kind, un);
            }
        }
        // include!·생성 파일의 자유 정의 — 소속 모듈 정점으로 귀속한다.
        if !matches!(
            self.push(module_path(db, f.module(db)), kind, false, un),
            Pushed::Miss
        ) {
            return;
        }
        self.st.external += 1;
    }

    /// 생성 impl 메서드 호출 — 메서드 정점이 없으므로 impl 대상 타입 정점을
    /// 가리킨다(튜플 구조체 생성자 호출이 구조체 정점을 가리키는 것과 같다).
    /// 블록 지역 타입의 생성 impl은 정규 ID가 같은 이름의 정점과 충돌할 수
    /// 있으므로 외부로 보낸다.
    fn emit_impl_owner(&mut self, i: Impl, kind: EdgeKind, un: bool) {
        let Some(adt) = i.self_ty(self.db()).as_adt() else {
            self.st.external += 1;
            return;
        };
        if adt_block_local(self.sema, adt) {
            self.st.external += 1;
            return;
        }
        let id = adt_id(self.db(), adt);
        if let Pushed::Miss = self.push(id, kind, false, un) {
            self.st.external += 1;
        }
    }

    /// 함수 정점으로의 확정 간선 — 정규 ID가 실제 선언과 일치하고 정점이
    /// 있어야 확정이다. 정점이 없거나 생성·충돌 정의면 외부다.
    fn emit_fn_vertex(&mut self, f: Function, kind: EdgeKind, un: bool) {
        match fn_id(self.sema, f) {
            Some(id) if syn_backed(self.sites, f, &id) => {
                if let Pushed::Miss = self.push(id, kind, false, un) {
                    self.st.external += 1;
                }
            }
            _ => self.st.external += 1,
        }
    }

    /// 트레이트 정의 메서드 호출 — 수신자가 dyn/제네릭이라 실제 대상을 못
    /// 고른다. 워크스페이스의 impl 후보(빌드 때 만든 표)와 기본 구현
    /// 정의점에 추정 간선을 단다 — "살아 있다" 편향은 syn 팬아웃과 같은
    /// 계약이다.
    fn trait_matrix(&mut self, t: Trait, def_fn: Function, kind: EdgeKind, un: bool) {
        self.st.trait_sites += 1;
        let db = self.db();
        if let Some(id) = fn_id(self.sema, def_fn) {
            if let Pushed::Miss = self.push(id, kind, true, un) {
                self.st.external += 1;
            }
        }
        let key = (trait_id(db, t), def_fn.name(db).as_str().to_string());
        let Some(candidates) = self.matrix.get(&key) else {
            return;
        };
        for cand in candidates {
            // 메서드 정점이 있으면 그쪽, 없으면(생성 impl) 타입 정점으로.
            // SelfEdge는 조용히 버려지는 정상 경로다 — 미스로 세지 않는다.
            let hit = [cand.method.as_ref(), cand.owner.as_ref()]
                .into_iter()
                .flatten()
                .any(|id| {
                    matches!(
                        self.push(id.clone(), kind, true, un),
                        Pushed::Yes | Pushed::SelfEdge
                    )
                });
            // 둘 다 없으면(blanket·원시 타입 impl) 해석됐지만 그래프에
            // 표현할 정점이 없다 — 디스패치 후보 전용으로 따로 센다.
            if !hit {
                self.st.unrepresentable += 1;
            }
        }
    }

    /// ra도 못 잡은 메서드 호출 — syn과 같은 이름 팬아웃 폴백.
    /// 후보가 하나도 없으면 간선 없는 미해석이다 — 세지 않으면
    /// limitation이 거짓말을 한다.
    fn fan_out(&mut self, name: Option<&str>, un: bool) {
        let Some(name) = name else {
            self.st.unresolved += 1;
            return;
        };
        match self.method_index.get(name) {
            Some(ids) => {
                for mid in ids {
                    self.push(mid.clone(), EdgeKind::Call, true, un);
                }
                self.st.fanned += 1;
            }
            None => self.st.unresolved += 1,
        }
    }

    /// `f()`·`T::assoc()`·`S(..)` — 경로 호출. 호출자가 지역 바인딩이면
    /// (`let f = helper; f()`) 경로 해석은 Local을 주므로 callable 해석이
    /// 실제 대상(fn 아이템)까지 따라간다.
    fn call(&mut self, ce: &ast::CallExpr, un: bool) {
        let Some(func) = ce.expr() else { return };
        if let ast::Expr::PathExpr(pe) = &func {
            if let Some(path) = pe.path() {
                match self.sema.resolve_path(&path) {
                    Some(PathResolution::Local(_)) => {}
                    _ => {
                        self.emit_path(path, EdgeKind::Call, un);
                        return;
                    }
                }
            }
        }
        let Some(callable) = self.sema.resolve_expr_as_callable(&func) else {
            // 호출 대상이 완전히 불투명 — 간선 없이 미해석으로만 센다.
            self.st.unresolved += 1;
            return;
        };
        match callable.kind() {
            ra_ap_hir::CallableKind::Function(f) => {
                self.emit_function(f, EdgeKind::Call, un, false)
            }
            ra_ap_hir::CallableKind::TupleStruct(s) => {
                // 블록 지역 튜플 구조체의 정규 ID는 같은 이름의 정점과 충돌한다.
                if adt_block_local(self.sema, Adt::Struct(s)) {
                    self.st.external += 1;
                } else {
                    let id = adt_id(self.db(), Adt::Struct(s));
                    if let Pushed::Miss = self.push(id, EdgeKind::Call, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            ra_ap_hir::CallableKind::TupleEnumVariant(v) => {
                if adt_block_local(self.sema, Adt::Enum(v.parent_enum(self.db()))) {
                    self.st.external += 1;
                } else {
                    let id = enum_id(self.db(), v.parent_enum(self.db()));
                    if let Pushed::Miss = self.push(id, EdgeKind::References, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            // 클로저·fn 포인터·Fn 트레이트 객체 — 정적 해석 밖.
            _ => self.st.unresolved += 1,
        }
    }

    /// `S { .. }` 구조체 리터럴 — 생성자 참조다(syn과 같은 종류).
    fn record(&mut self, re: &ast::RecordExpr, un: bool) {
        if let Some(path) = re.path() {
            self.emit_path(path, EdgeKind::References, un);
        }
    }

    /// 호출이 아닌 경로 참조 — `filter_map(f)` 같은 함수 값·상수·타입.
    /// 지역 바인딩은 ra가 구분해준다 — 이름이 같아도 오인하지 않는다.
    fn path_ref(&mut self, pe: &ast::PathExpr, un: bool) {
        // 호출자 위치의 경로는 call()이 Call로 처리했다 — 이중 계수 금지.
        if pe
            .syntax()
            .parent()
            .is_some_and(|p| ast::CallExpr::cast(p).is_some())
        {
            return;
        }
        if let Some(path) = pe.path() {
            self.emit_path(path, EdgeKind::References, un);
        }
    }

    /// 경로 해석 결과를 간선으로. MethodCallExpr 안의 자체 참조는 위에서
    /// 처리했으므로 여기는 순수 경로다.
    fn emit_path(&mut self, path: ast::Path, kind: EdgeKind, un: bool) {
        match self.sema.resolve_path(&path) {
            Some(PathResolution::Def(d)) => self.emit_def(d, kind, un),
            // `Self::x` — impl의 self 타입으로의 참조다.
            Some(PathResolution::SelfType(imp)) => {
                // 블록 지역 impl의 self 타입도 정규 ID 충돌이 가능하다.
                if imp
                    .self_ty(self.db())
                    .as_adt()
                    .is_some_and(|a| adt_block_local(self.sema, a))
                    || block_local_def(self.sema, imp)
                {
                    self.st.external += 1;
                } else if let Some(adt) = imp.self_ty(self.db()).as_adt() {
                    let id = adt_id(self.db(), adt);
                    if let Pushed::Miss = self.push(id, EdgeKind::References, false, un) {
                        self.st.external += 1;
                    }
                }
            }
            // 지역 바인딩·타입 파라미터·derive 헬퍼 — 그래프 정점이 아니다.
            Some(_) => {}
            None => self.st.unresolved += 1,
        }
    }

    /// ModuleDef → 정규 ID → 간선. 해석됐는데 정점이 없으면 외부다.
    fn emit_def(&mut self, d: ModuleDef, kind: EdgeKind, un: bool) {
        let db = self.db();
        let id = match d {
            ModuleDef::Function(f) => return self.emit_function(f, kind, un, false),
            ModuleDef::Adt(a) => adt_id(db, a),
            ModuleDef::EnumVariant(v) => enum_id(db, v.parent_enum(db)),
            ModuleDef::Const(c) => match const_id(db, c) {
                Some(id) => id,
                // 연관 상수 — 정점 스킴이 없어 외부로 센다.
                None => {
                    self.st.external += 1;
                    return;
                }
            },
            ModuleDef::Static(s) => static_id(db, s).unwrap_or_default(),
            ModuleDef::Trait(t) => trait_id(db, t),
            ModuleDef::TypeAlias(t) => alias_id(db, t),
            ModuleDef::Module(m) => module_path(db, m),
            ModuleDef::Macro(m) => macro_id(db, m),
            // prelude 내장 타입 — 그래프 정점이 아니다.
            ModuleDef::BuiltinType(_) => return,
        };
        // 블록 지역 정의의 정규 ID는 같은 이름의 정점과 충돌할 수 있다 —
        // 확인된 간선을 만들지 않고 그래프 밖으로 본다.
        if self.def_block_local(d) {
            self.st.external += 1;
            return;
        }
        if let Pushed::Miss = self.push(id, kind, false, un) {
            // 정점이 없는 정의 — include!·생성 파일 안의 정의면 소속 모듈
            // 정점으로 귀속해 사용처를 보존한다(syn이 모듈 경로로 잡던 것).
            // 크레이트 루트 include!면 크레이트 정점으로 간다 — 자기 간선은
            // push가 알아서 걸러준다.
            if let Some(module) = d.module(db).map(|m| module_path(db, m)) {
                if matches!(self.push(module, kind, false, un), Pushed::Yes) {
                    return;
                }
            }
            self.st.external += 1;
        }
    }

    /// 정의가 블록 스코프에 사는가 — 자기 구문이 fn 본문 안이거나,
    /// 소속 모듈이 블록 모듈(fn 안 `#[path] mod` 같은)이면 지역이다.
    /// 후자는 원본 파일을 공유하는 지역 모듈의 아이템이 모듈 레벨 정점을
    /// 훔치는 것을 막는다.
    fn def_block_local(&self, d: ModuleDef) -> bool {
        let sema = self.sema;
        let own = match d {
            ModuleDef::Adt(Adt::Struct(x)) => block_local_def(sema, x),
            ModuleDef::Adt(Adt::Enum(x)) => block_local_def(sema, x),
            ModuleDef::Adt(Adt::Union(x)) => block_local_def(sema, x),
            ModuleDef::EnumVariant(x) => block_local_def(sema, x),
            ModuleDef::Const(x) => block_local_def(sema, x),
            ModuleDef::Static(x) => block_local_def(sema, x),
            ModuleDef::Trait(x) => block_local_def(sema, x),
            ModuleDef::TypeAlias(x) => block_local_def(sema, x),
            ModuleDef::Macro(x) => block_local_def(sema, x),
            _ => false,
        };
        own || d
            .module(self.db())
            .is_some_and(|m| module_is_local(self.db(), m))
    }

    /// `name!()` — 크레이트 안 매크로면 call 간선, 외부면 실측 생략.
    /// 그리고 확장 트리로 내려가 확장 안의 호출·참조를 수확한다 —
    /// 이것이 syn의 토큰 파싱 폴백을 대체하는 근본적 개선이다.
    fn macro_call(&mut self, mac: &ast::MacroCall, un: bool, depth: usize) {
        if let Some(m) = self.sema.resolve_macro_call(mac) {
            if m.kind(self.db()) == ra_ap_hir::MacroKind::ProcMacro {
                self.st.proc_macros += 1;
            }
            let id = macro_id(self.db(), m);
            if let Pushed::Miss = self.push(id, EdgeKind::Call, false, un) {
                // 정점이 없는 매크로 — proc 매크로 크레이트처럼 정점이 안
                // 생기는 선언이면 소속 모듈(크레이트 루트) 정점으로 귀속해
                // 실사용을 보존한다. 그래도 없으면 syn과 같은 외부 매크로다.
                let module = module_path(self.db(), m.module(self.db()));
                let own = self.owner.split("::").next().unwrap_or(self.owner);
                if module == own
                    || matches!(self.push(module, EdgeKind::Call, false, un), Pushed::Miss)
                {
                    self.st.ext_macros += 1;
                }
            }
        } else {
            self.st.ext_macros += 1;
        }
        if depth >= MAX_EXPANSION_DEPTH {
            self.st.unexpanded += 1;
            return;
        }
        // expand는 ValueResult — 구문이 나와도 오류가 실리면 부분 확장이다.
        // 조각 안의 호출은 진짜라 걷되, 성공으로는 세지 않는다.
        let Some(call_id) = self.sema.to_def(mac) else {
            self.st.unexpanded += 1;
            return;
        };
        let res = self.sema.expand(call_id);
        if res.err.is_some() {
            self.st.unexpanded += 1;
        } else {
            self.st.expanded += 1;
        }
        self.walk(&res.value, un, depth + 1);
    }
}
