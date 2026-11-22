//! 모듈 트리와 경로 해석.
//!
//! Rust의 `mod` 선언만이 파일→모듈의 권위다 — 디렉터리를 훑어 모듈을
//! "발견"하지 않는다(발견하면 선언되지 않은 orphan 파일이 정점으로 새고,
//! 그건 유령 정점이다). orphan 파일은 limitation으로 센다.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// 의존 해석 표 — 코드가 보는 라이브러리 이름을 크레이트 정점으로 연결한다.
/// 스코프는 의존을 선언한 크레이트(타깃 이름)다 — 같은 별칭을 멤버마다
/// 다른 패키지에 물릴 수 있어 전역 맵이면 last-wins로 오염된다.
/// 호출자가 빈 표를 넘기면 외부 경로는 전부 미해석이다(기본 동작).
#[derive(Debug, Default)]
pub struct DepCrates {
    /// 선언 크레이트(타깃 이름) → lib 별칭 → 타깃.
    scoped: BTreeMap<String, BTreeMap<String, DepTarget>>,
    /// 정점으로 존재하는 외부 크레이트 정점 ID — `use` 임포트가 외부
    /// 정점을 가리킬 때 후속 경로를 그 정점으로 붕괴하는 데 쓴다.
    /// 멤버 크레이트는 없다 — 멤버 안은 실제 정점으로 걸을 수 있다.
    pub external: BTreeSet<String>,
}

/// 별칭이 가리키는 것 — 외부 크레이트 정점 또는 멤버 크레이트 루트.
#[derive(Debug)]
pub struct DepTarget {
    /// 정점 ID — 외부는 패키지 이름, 멤버는 루트 모듈(타깃 이름).
    /// 패키지 이름과 lib 타깃 이름이 다른 멤버도 있으므로 패키지
    /// 이름을 그대로 쓰지 않는다 — 루트 정점이 기준이다.
    pub vertex: String,
    /// 워크스페이스 멤버면 true — 붕괴하지 않고 루트부터 계속 걷는다.
    pub member: bool,
}

impl DepCrates {
    /// 새 빈 표.
    pub fn new() -> DepCrates {
        DepCrates::default()
    }

    /// 선언 크레이트의 스코프에 별칭을 심는다. 외부 타깃은 external
    /// 집합에도 들어간다 — 임포트 붕괴 후속 해석이 그 집합을 본다.
    pub fn insert(&mut self, scope: String, alias: String, target: DepTarget) {
        if !target.member {
            self.external.insert(target.vertex.clone());
        }
        self.scoped.entry(scope).or_default().insert(alias, target);
    }

    /// `from`이 속한 크레이트가 선언한 별칭을 찾는다.
    fn lookup(&self, from: &str, alias: &str) -> Option<&DepTarget> {
        self.scoped
            .get(crate_of(from).as_str())
            .and_then(|m| m.get(alias))
    }
}

/// 이름이 보이는 범위 — rustc 가시성을 모듈 경로로 정규화한 것. 글롭(`use m::*`)은 들이는 모듈에서
/// 보이는 이름만 들인다: 비공개는 선언한 모듈과 그 자손, `pub(super)`는 부모와 그 자손, `pub(crate)`는
/// 그 크레이트 안, `pub`은 어디서나.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum Scope {
    /// `pub` — 어디서나 보인다.
    #[default]
    Public,
    /// 이 모듈과 그 자손 모듈에서만 보인다 — `pub(crate)`는 크레이트 루트, `pub(super)`는 부모,
    /// 비공개·`pub(self)`는 선언한 모듈, `pub(in path)`는 그 모듈.
    Within(String),
}

impl Scope {
    /// 모듈 `module`에서 보이는가.
    pub fn includes(&self, module: &str) -> bool {
        match self {
            Scope::Public => true,
            Scope::Within(scope) => {
                module == scope
                    || module
                        .strip_prefix(scope.as_str())
                        .is_some_and(|rest| rest.starts_with("::"))
            }
        }
    }

    /// 두 범위가 함께 허락하는 범위 — 글롭이 재노출하는 이름은 글롭의 가시성과 원래 가시성 가운데
    /// 좁은 쪽으로 보인다. 같은 모듈에서 보이는 두 범위는 한 갈래 위에 있어 더 깊은 쪽이다.
    fn narrower(&self, other: &Scope) -> Scope {
        match (self, other) {
            (Scope::Public, other) => other.clone(),
            (this, Scope::Public) => this.clone(),
            (Scope::Within(a), Scope::Within(b)) => {
                Scope::Within(if a.len() >= b.len() { a } else { b }.clone())
            }
        }
    }

    /// 둘 중 넓은 범위 — 같은 이름의 아이템·임포트가 여럿이면 가장 넓게 보이는 쪽으로 센다.
    fn wider(&self, other: &Scope) -> Scope {
        match (self, other) {
            (Scope::Public, _) | (_, Scope::Public) => Scope::Public,
            (Scope::Within(a), Scope::Within(b)) => {
                Scope::Within(if a.len() <= b.len() { a } else { b }.clone())
            }
        }
    }
}

/// `use` 임포트 하나 — 해석된 정규 경로와 `#[cfg]` 조건, 가시 범위.
/// 조건이 있으면 그 빌드에서만 존재하는 임포트다 — 그 이름으로 해석한 간선(`uses`, 본문·시그니처·필드·속성)도
/// 조건을 물려받는다([`Resolved::cfg`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Import {
    /// 정규 ID (`crate::mod::item`).
    pub target: String,
    /// `#[cfg(...)]` 조건 토큰 — 없으면 무조건 임포트. 그 `use`의 조건에 거쳐 온 재노출(`pub use`·글롭)의 조건을
    /// 함께 싣는다.
    pub cfg: Option<String>,
    /// 이 임포트(재노출)가 보이는 범위 — 다른 모듈의 글롭은 그 모듈에서 보이는 임포트만 펼친다.
    pub scope: Scope,
}

/// 이름공간 — rustc는 같은 이름을 타입(모듈·struct·enum·union·trait·type·크레이트), 값(fn·const·static·
/// 튜플·유닛 struct 생성자), 매크로(`macro_rules!`·derive 등록 이름)에 따로 둔다. `fn harvest`와
/// `use crate::harvest`가 한 모듈에 함께 있을 수 있는 이유다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Ns {
    /// 타입 이름공간 — 경로 접두사(`name::item`)와 타입 자리, 구조체 리터럴.
    Type,
    /// 값 이름공간 — 호출·값 경로.
    Value,
    /// 매크로 이름공간 — 매크로 호출·속성·derive 인자.
    Macro,
}

impl Ns {
    /// 모든 이름공간 — 순회 순서이기도 하다.
    pub const ALL: [Ns; 3] = [Ns::Type, Ns::Value, Ns::Macro];
}

/// 이름 하나가 이름공간마다 묶인 것 — 비어 있는 칸은 그 이름공간에 그 이름이 없다는 뜻이다.
///
/// 이름공간을 정하지 못한 것은 세 칸이 아니라 [`PerNs::undetermined`]에 둔다. 세 칸에 똑같이 두면 그 이름공간에
/// 정말 묶인 것과 구별되지 않아, 같은 이름의 로컬 trait 옆에 둔 `pub use derive_crate::Foo`(실제로는 매크로
/// 이름공간뿐이다)가 값 자리의 `Foo`까지 차지했다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PerNs<T> {
    /// 타입 이름공간.
    pub types: Option<T>,
    /// 값 이름공간.
    pub values: Option<T>,
    /// 매크로 이름공간.
    pub macros: Option<T>,
    /// 어느 이름공간인지 정하지 못한 것 — 아이템 표가 이름공간을 모르는 명시 `use`(외부 크레이트로 붕괴한 경로,
    /// std 경로, 매크로가 만든 이름)와 그것을 글롭·재노출로 다시 들인 것이다. 해석은 같은 이름의 로컬 아이템이나
    /// 이름공간이 정해진 칸이 없을 때만 이것을 쓴다([`lookup`]). rustc에서 명시 `use`는 같은 이름공간의 로컬
    /// 아이템·다른 명시 `use`와 공존하지 못하므로(E0255·E0252) 그런 것이 있으면 이 `use`는 다른 이름공간에 있다.
    /// `#[cfg]`로 갈린 대안 임포트(`#[cfg(feature = "pl")] use crate::pl::Condvar;`와 `#[cfg(not(..))] use
    /// std::sync::Condvar;`)는 같은 이름공간인데 rustc가 둘을 한 빌드에서 보지 않을 뿐이라 이 근거가 닿지 않는다 —
    /// 그때도 소스 순서와 무관하게 정해진 쪽(워크스페이스)이 이기고, std 쪽은 그래프 밖이라 간선이 없다. 그 이름으로
    /// 해석한 간선은 워크스페이스 쪽 임포트의 cfg(`feature = "pl"`)를 싣는다([`Resolved::cfg`]).
    pub undetermined: Option<T>,
}

impl<T> Default for PerNs<T> {
    fn default() -> Self {
        PerNs {
            types: None,
            values: None,
            macros: None,
            undetermined: None,
        }
    }
}

impl<T> PerNs<T> {
    /// 이름공간을 정하지 못한 것 하나만 묶는다 — [`PerNs::undetermined`].
    pub fn undetermined_only(value: T) -> PerNs<T> {
        PerNs {
            undetermined: Some(value),
            ..PerNs::default()
        }
    }

    /// 이름공간 `ns`의 칸.
    pub fn get(&self, ns: Ns) -> Option<&T> {
        match ns {
            Ns::Type => self.types.as_ref(),
            Ns::Value => self.values.as_ref(),
            Ns::Macro => self.macros.as_ref(),
        }
    }

    /// 이름공간 `ns`의 칸(쓰기).
    pub fn slot(&mut self, ns: Ns) -> &mut Option<T> {
        match ns {
            Ns::Type => &mut self.types,
            Ns::Value => &mut self.values,
            Ns::Macro => &mut self.macros,
        }
    }

    /// 이름공간이 정해진 칸을 [`Ns::ALL`] 순서로 — [`PerNs::undetermined`]는 빠진다.
    pub fn iter(&self) -> impl Iterator<Item = (Ns, &T)> {
        Ns::ALL
            .into_iter()
            .filter_map(move |ns| self.get(ns).map(|v| (ns, v)))
    }

    /// 묶인 것 전부 — 이름공간이 정해진 칸([`Ns::ALL`] 순서) 다음에 [`PerNs::undetermined`]다. 이름공간을
    /// 묻지 않는 소비자(모듈 `uses` 의존 증거)용이다.
    pub fn bound(&self) -> impl Iterator<Item = &T> {
        self.iter()
            .map(|(_, v)| v)
            .chain(self.undetermined.as_ref())
    }

    /// 이름공간이 정해진 칸에 묶인 것이 있는가.
    pub fn has_determined(&self) -> bool {
        self.iter().next().is_some()
    }

    /// 아무것도 묶이지 않았는가([`PerNs::undetermined`] 포함).
    pub fn is_empty(&self) -> bool {
        !self.has_determined() && self.undetermined.is_none()
    }
}

/// 한 `use` 이름이 한 이름공간에서 묶이는 것 — 그래프 정점으로 해석된 임포트, 또는 그래프 밖(std·core·alloc,
/// `--deps` 없는 외부 크레이트 등)이라 해석되지 않는 이름과 그 가시 범위. 해석되지 않는 이름도 같은 이름공간의
/// 글롭 임포트를 가리고, 그 이름의 해석을 크레이트 루트 같은 다른 후보로 넘기지 않는다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Binding {
    /// 정점으로 해석된 임포트.
    Resolved(Import),
    /// 서로 다른 정점을 가리키는 대안 임포트 둘 이상 — 같은 이름공간에 같은 이름을 들이는 명시 `use` 둘
    /// (`#[cfg(unix)] use crate::unix::Sys;`와 `#[cfg(windows)] use crate::windows::Sys;`)이나 글롭 둘
    /// (`#[cfg(unix)] pub use self::unix::*;`와 `#[cfg(windows)] pub use self::windows::*;`)이 같은 이름을 들였다.
    /// rustc는 한 빌드의 한 이름공간에 그런 둘을 두지 않으므로(명시 `use`는 E0252, 글롭 이름은 쓰면 E0659)
    /// 둘은 서로 다른 빌드의 것이다 — 모든 빌드의 합집합을 보는 그래프는 둘 다 남기고, 간선은 각 임포트의 cfg를
    /// 싣는다. 하나만 남기면(예전에는 소스 순서상 뒤의 것) 다른 빌드의 대상과 그 경로로만 쓰는 아이템이 거짓
    /// unreachable이 됐다. 이름을 묶지 않는 `use T as _`는 rustc에서도 여럿이 공존하므로, 한 모듈의 `_` 칸에 모인
    /// 그것들도 같은 모양으로 모두 남는다(모듈 `uses` 의존 증거다). 첫 원소가 소스 순서상 뒤의 것이다 — 대상 하나를
    /// 고르는 해석([`ModTree::resolve`])이 예전처럼 그것을 고른다. 원소는 둘 이상이고 대상이 서로 다르다.
    Alternatives(Vec<Import>),
    /// 해석되지 않는 임포트의 가시 범위.
    Unresolved(Scope),
}

impl Binding {
    /// 해석된 임포트 전부 — 대상 하나를 고르는 해석이 따르는 것이 첫 원소다. 해석되지 않는 이름이면 비어 있다.
    pub fn imports(&self) -> &[Import] {
        match self {
            Binding::Resolved(imp) => std::slice::from_ref(imp),
            Binding::Alternatives(imps) => imps,
            Binding::Unresolved(_) => &[],
        }
    }

    /// 해석된 임포트 목록을 칸 값으로 — 하나면 [`Binding::Resolved`], 둘 이상이면 [`Binding::Alternatives`]다.
    /// 비어 있으면 `None`이다.
    fn of_imports(mut imports: Vec<Import>) -> Option<Binding> {
        match imports.len() {
            0 => None,
            1 => imports.pop().map(Binding::Resolved),
            _ => Some(Binding::Alternatives(imports)),
        }
    }
}

/// 경로 하나를 해석한 대상 — 정점 ID와, 그 정점에 닿으려고 거친 `use` 임포트(재노출·글롭 포함)들의 `#[cfg]`를
/// 모두 합친 조건. 그 임포트가 없는 빌드에서는 그 이름으로 그 정점에 닿을 수 없으므로 그 경로로 만든 간선도 같은
/// 조건 아래서만 성립한다 — `#[cfg(feature = "pl")] use crate::pl::Condvar;` 옆의 `Condvar::new()`는
/// `feature = "pl"`일 때만 `pl::Condvar::new`를 부른다. 로컬 아이템과 `crate::`·`self::`·`super::` 경로로 바로
/// 닿은 정점은 조건이 없다(정점 자신의 cfg는 정점에 있다).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 정규 ID (`crate::mod::item`), 또는 외부 크레이트 정점.
    pub target: String,
    /// 거친 임포트들의 `#[cfg]`를 합친 조건 — 없으면 조건 없이 닿는다.
    pub cfg: Option<String>,
}

impl Resolved {
    /// 임포트를 거치지 않고 닿은 정점(로컬 아이템, `crate::`·`self::`·`super::`, 크레이트 루트) — 조건이 없다.
    fn plain(target: String) -> Resolved {
        Resolved { target, cfg: None }
    }

    /// 경로의 첫 세그먼트가 묶인 임포트 `imp`로 닿은 정점 — 그 임포트의 조건이다.
    fn via(imp: &Import) -> Resolved {
        Resolved {
            target: imp.target.clone(),
            cfg: imp.cfg.clone(),
        }
    }

    /// 지금 닿은 모듈에서 임포트 `imp`(재노출)를 따라간 정점 — 거쳐 온 조건에 그 임포트의 조건을 더한다.
    fn through(&self, imp: &Import) -> Resolved {
        Resolved {
            target: imp.target.clone(),
            cfg: with_import_cfg(&self.cfg, &imp.cfg),
        }
    }

    /// 임포트를 거치지 않고 한 단계 내려간 정점(자식 모듈·아이템) — 조건은 그대로다.
    fn at(self, target: String) -> Resolved {
        Resolved {
            target,
            cfg: self.cfg,
        }
    }
}

/// [`ModTree::resolve_use`]가 한 이름공간에서 찾은 대상 하나 — `target`이 `None`이면 그래프 밖 이름이고, `cfg`는
/// 거쳐 온 재노출들의 조건이다(그 `use` 자신의 조건은 칸을 만들 때 더한다).
#[derive(Debug, Clone)]
struct UseTarget {
    target: Option<String>,
    cfg: Option<String>,
}

/// 경로의 마지막 세그먼트를 어느 이름공간에서 찾는가. 접두사 세그먼트는 늘 타입 이름공간이다.
#[derive(Debug, Clone, Copy)]
enum Want {
    /// 자리를 모른다 — 로컬 아이템이 먼저고, 임포트는 이름공간이 정해진 해석된 것(값 → 타입 → 매크로), 이름공간을
    /// 정하지 못한 해석된 것 순이다.
    Any,
    /// 그 이름공간이 먼저다. 그 이름공간에 묶인 것이 없으면 [`Want::Any`]로 넘어간다 — 아이템 표가 모르는
    /// 이름공간(매크로가 만든 아이템, 배리언트 등)에서도 예전처럼 찾아 보존을 좁히지 않는다.
    Prefer(Ns),
}

/// 모듈 하나에서 이름 하나를 찾은 결과.
enum Found<'a> {
    /// 그 모듈이 선언한 아이템(자식 모듈 포함) — `모듈::이름`이다.
    Local,
    /// 모든 선언이 `#[cfg]`를 단 로컬 아이템(먼저)과, 그 아이템이 없는 빌드에서 그 이름이 가리키는 같은 이름공간의
    /// 임포트들([`Module::local_alternatives`]) — 비어 있지 않다.
    LocalOr(&'a [Import]),
    /// `use`로 들인 정점 — 비어 있지 않고, 첫 원소가 대상 하나를 고르는 해석이 따르는 것이다
    /// ([`Binding::imports`]). 둘 이상이면 `#[cfg]`로 갈린 대안이다.
    Import(&'a [Import]),
    /// 그래프 밖에서 들인 이름 — 해석을 멈춘다.
    Unresolved,
    /// 그 모듈에 없는 이름.
    Unbound,
}

/// 모듈 하나 — 파일 단위든 인라인 `mod x {}`든 동일하게 표현한다.
/// 경로는 트리의 맵 키이므로 필드로 중복 저장하지 않는다.
#[derive(Debug, Clone)]
pub struct Module {
    /// 이 모듈의 선언이 있는 파일(인라인 모듈은 부모와 같은 파일).
    pub file: PathBuf,
    /// 루트 모듈을 공유하는 다른 타깃의 루트 파일 — lib와 같은 이름의
    /// bin이 있으면 두 크레이트 인스턴스가 한 모듈 경로를 공유한다
    /// (bin의 `use pkg::x`는 lib 네임스페이스를 가리키므로 합집합이 맞다).
    pub extra_files: Vec<PathBuf>,
    /// 파일 루트 모듈인가 — false면 부모 AST 안의 인라인 `mod x {}`다.
    pub file_module: bool,
    /// `pub mod`로 선언됐는가 — 크레이트 루트는 항상 true다.
    pub public: bool,
    /// `#[cfg(...)]` 조건 토큰 — `mod` 선언에 붙은 것만(조상 조건은 조상 정점에).
    pub cfg: Option<String>,
    /// 이 모듈 안에 선언된 자식 모듈의 기준 디렉터리 — rustc 규칙:
    /// 루트·`mod.rs`·`#[path]`로 로드된 파일은 파일이 놓인 디렉터리,
    /// 일반 `name.rs`는 `name/` 디렉터리, 인라인 `mod m {}`은 선언
    /// 문맥의 기준 디렉터리에 자기 세그먼트(이름 또는 `#[path]` 값)를
    /// 이어붙인 디렉터리다.
    pub dir: PathBuf,
    /// 직접 선언된 아이템 이름들(모듈 스코프 해석용).
    pub items: BTreeSet<String>,
    /// 그중 값 이름공간의 아이템(fn·const·static, 생성자를 겸하는 튜플·유닛 struct) — `mod util`과
    /// `fn util`처럼 이름공간이 다른 두 아이템이 한 경로를 공유할 때 값 자리 해석이 fn을 고른다.
    pub value_items: BTreeSet<String>,
    /// 그중 타입 이름공간의 아이템(mod·struct·enum·union·trait·type) — 경로 접두사 자리
    /// (`name::item`)에서 같은 이름의 임포트를 가리는 로컬 아이템이다.
    pub type_items: BTreeSet<String>,
    /// 그중 매크로 이름공간의 아이템(이름 있는 `macro_rules!`).
    pub macro_items: BTreeSet<String>,
    /// 이름 → 이름공간 → 그 이름공간의 선언마다 단 `#[cfg]` 조건(선언이 모두 `#[cfg]`를 달았을 때만 있다). 그런
    /// 로컬 아이템은 조건이 아닌 빌드에 없으므로 같은 이름의 임포트가 그 빌드의 대안일 수 있다
    /// ([`Module::local_alternatives`]). `#[cfg] mod`도 같다.
    pub cfg_gated: BTreeMap<String, BTreeMap<Ns, Vec<String>>>,
    /// 이름 → 이름공간마다 `#[cfg]`를 단 로컬 아이템([`Module::cfg_gated`])이 없는 빌드에서 그 이름이 가리키는 임포트.
    /// 같은 이름공간의 명시 `use`는 늘 대안이다 — rustc는 한 빌드에서 로컬 아이템과 명시 `use`가 이름공간을 나누지
    /// 못하게 하니(E0255) 그 `use`는 아이템이 없는 빌드의 것이다(`#[cfg(feature = "x")] fn sum()` 옆
    /// `#[cfg(not(feature = "x"))] use scalar::sum;`). 글롭 이름은 로컬 아이템 옆에 합법적으로 있으면서 가려지기만
    /// 하므로, 아이템의 선언이 하나일 때만 그 조건의 부정(`not (..)`)을 달고 대안이 된다 — 선언이 여럿이면
    /// (`#[cfg(unix)] fn sys`와 `#[cfg(not(unix))] fn sys`) 모든 빌드를 덮을 수 있어 글롭 이름은 대안이 아니다.
    /// [`fill_imports`]가 채운다.
    pub local_alternatives: BTreeMap<String, PerNs<Binding>>,
    /// 선언 아이템(자식 모듈 포함)이 보이는 범위 — 이름 → 같은 이름 아이템 가운데 가장 넓은 범위.
    /// 없는 이름은 `pub`으로 본다. 같은 모듈의 같은 이름 아이템은 이름공간이 달라도 정점 ID가 같다.
    pub item_scopes: BTreeMap<String, Scope>,
    /// `use` 임포트 맵: 마지막 세그먼트(또는 as 이름) → 이름공간마다 묶인 것. 명시 `use`는 대상이 차지한
    /// 이름공간에만 묶이고, 글롭은 대상 모듈의 이름을 이름공간마다 따로 들인다. 명시 임포트는 같은
    /// 이름공간의 글롭만 가린다(rustc와 같다). derive 등록 이름은 매크로 이름공간에 있다. 이름공간을 정하지
    /// 못한 명시 `use`는 [`PerNs::undetermined`]에 있고, 어느 이름공간을 차지하는지 모르니 같은 이름의 글롭을
    /// 모두 가린다. 같은 칸에 대상이 다른 명시 `use` 둘(또는 글롭 둘)이 오면 둘 다 남는다
    /// ([`Binding::Alternatives`]).
    pub imports: BTreeMap<String, PerNs<Binding>>,
    /// 이 모듈이 글롭으로 들이는 모듈·외부 크레이트 정점(`use m::*`의 `m`) → 그 `use`의 `#[cfg]`. 그 글롭의
    /// 의존 증거(uses 간선, deps의 사용 증거)일 뿐 이름이 아니다 — 이름 해석과 다른 모듈의 글롭에 쓰지 않는다.
    pub glob_targets: BTreeMap<String, Option<String>>,
    /// 자식 모듈 이름 → 경로.
    pub children: BTreeMap<String, String>,
}

impl Module {
    /// 새 모듈 항목.
    pub fn new(file: PathBuf, file_module: bool, public: bool) -> Module {
        Module {
            dir: module_dir(&file),
            file,
            extra_files: Vec::new(),
            file_module,
            public,
            cfg: None,
            items: BTreeSet::new(),
            value_items: BTreeSet::new(),
            type_items: BTreeSet::new(),
            macro_items: BTreeSet::new(),
            cfg_gated: BTreeMap::new(),
            local_alternatives: BTreeMap::new(),
            item_scopes: BTreeMap::new(),
            imports: BTreeMap::new(),
            glob_targets: BTreeMap::new(),
            children: BTreeMap::new(),
        }
    }
}

/// 크레이트 하나의 모듈 트리 전체.
#[derive(Debug, Default, Clone)]
pub struct ModTree {
    /// 경로 → 모듈.
    pub modules: BTreeMap<String, Module>,
    /// 선언에 도달하지 못한 .rs 파일(orphan) — limitation으로 올린다.
    pub orphan_files: Vec<PathBuf>,
    /// 정점 없이 건너뛴 `mod` 선언 — 모듈 경로 → 사유. limitation으로 올린다.
    /// 경로가 키라 병합 루트(lib+같은 이름 bin)를 두 타깃이 다시 훑어도 한 번만 센다.
    pub skipped_mods: BTreeMap<String, SkippedMod>,
    /// 2015 판 타깃의 루트 모듈(크레이트) 경로 — 호출자가 채운다. 이 해석기는 2018 판 이후의 경로 규칙이라, 2015 판의
    /// 크레이트 기준 `use` 경로(`use foo::bar`가 `crate::foo::bar`)를 모른다. 그런 크레이트의 모듈에서는
    /// [`fill_imports_settled`]가 앞 바퀴의 대상을 거둬 지우지 않는다(거둔 쪽이 2015 판의 제 대상일 수 있다).
    pub edition_2015: BTreeSet<String>,
    /// [`fill_imports_settled`]가 남긴 것 — 앞 바퀴의 대상을 거둔 모듈이 있어 빈 표에서 다시 수렴시켰는데
    /// [`MAX_IMPORT_PASSES`]번 다시 쌓아도 앞 바퀴의 대상을 거두는 모듈 수. 그 모듈의 임포트는 거둔 대상을 대안으로 남길 수
    /// 있다 — limitation으로 올린다.
    pub stale_import_modules: usize,
    /// [`fill_imports_settled`]가 남긴 것 — 루트가 아닌 모듈의 `use` 경로가 크레이트 루트가 선언한 이름으로 시작하는
    /// 2015 판 크레이트([`ModTree::edition_2015`]). 2015 판은 그 경로를 크레이트 루트부터 풀지만 이 해석기는 그 모듈부터
    /// 푼다(2018 판 규칙) — 그 경로로만 쓰는 아이템을 `dead`가 낼 수 있고 간선이 다른 아이템으로 갈 수 있다. limitation으로
    /// 올린다.
    pub edition_2015_crate_relative: BTreeSet<String>,
}

/// 모듈 경로 깊이(세그먼트 수) 상한이다. 순환은 조상 파일 검사가 끊지만, 그 검사를
/// 비껴가는 입력(정규화되지 않는 경로 등)에도 트리 확장이 끝나도록 둔 보조 상한이다.
/// 실제 크레이트의 모듈 중첩(수십 단계 미만)보다 넉넉하게 잡은 판단값이다.
pub const MAX_MODULE_DEPTH: usize = 512;

/// `mod` 선언을 정점 없이 건너뛴 사유 — 유령 정점 대신 실측으로 센다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkippedMod {
    /// 조상 모듈의 파일을 다시 싣는다(rustc의 circular modules). cfg로 막혀 rustc가
    /// 받는 크레이트도 있어 오류가 아니라 실측이다. 그대로 실으면 같은 파일이
    /// `m`, `m::m`, …으로 끝없이 늘어난다.
    Circular,
    /// 대상이 정규 파일이 아니다(장치·FIFO·디렉터리) — 읽으면 영원히 막히거나
    /// 끝없이 읽힌다.
    NotRegularFile,
    /// 모듈 경로가 [`MAX_MODULE_DEPTH`]를 넘는다.
    TooDeep,
}

/// 모듈 경로에서 크레이트(첫 세그먼트)를 뗀다.
pub fn crate_of(path: &str) -> String {
    path.split("::").next().unwrap_or(path).to_string()
}

/// 부모 모듈 경로 — 루트 모듈의 부모는 없다.
pub fn parent_of(path: &str) -> Option<String> {
    path.rsplit_once("::").map(|(p, _)| p.to_string())
}

/// `mod x;` 선언이 가리키는 파일을 찾는다: `x.rs` 또는 `x/mod.rs`,
/// `#[path = "..."]` 우선. 파일이 없으면 선언만 있고 내용이 없는 모듈이다 —
/// 유령 정점을 만들지 않기 위해 None을 돌려준다.
pub fn mod_file(dir: &Path, name: &str, path_attr: Option<&str>) -> Option<PathBuf> {
    if let Some(p) = path_attr {
        let f = dir.join(p);
        return f.exists().then(|| f.canonicalize().unwrap_or(f));
    }
    // r#은 식별자 표기이고 파일명 일부가 아니다. 명시적 #[path] 문자열은 위에서 그대로 쓴다.
    let name = name.strip_prefix("r#").unwrap_or(name);
    for cand in [
        dir.join(format!("{name}.rs")),
        dir.join(name).join("mod.rs"),
    ] {
        if cand.exists() {
            return Some(cand.canonicalize().unwrap_or(cand));
        }
    }
    None
}

/// 모듈의 파일이 있는 디렉터리 — `mod x;`의 상대 기준.
/// 루트(lib.rs/main.rs)와 `mod.rs`는 자기 디렉터리, 그 외 파일 `a.rs`는
/// `a/`가 자식 모듈 기준 디렉터리다.
pub fn module_dir(file: &Path) -> PathBuf {
    let dir = file.parent().unwrap_or(Path::new("."));
    match file.file_stem().and_then(|s| s.to_str()) {
        Some("lib") | Some("main") | Some("mod") => dir.to_path_buf(),
        Some(stem) => dir.join(stem),
        None => dir.to_path_buf(),
    }
}

impl ModTree {
    /// 모듈 경로에서 시작해 `use` 세그먼트를 해석한다.
    /// 반환값은 정규 ID. 해석 불가(prelude·매크로 생성 이름)면 None —
    /// 유령 정점을 만들지 않는 것이 계약이다. 외부 크레이트 경로는
    /// dep_crates가 비어 있지 않으면 크레이트 정점 ID로 붕괴한다 —
    /// `serde::de::X`는 `serde`다. 크레이트 안은 못 보지만 경계까지는
    /// 사실이다. 워크스페이스 멤버 별칭은 붕괴하지 않고 그 멤버의
    /// 실제 루트부터 걷는다 — 나머지 세그먼트가 진짜 정점이다.
    ///
    /// 마지막 세그먼트의 이름공간을 모르는 자리다 — 로컬 아이템이 먼저다. 자리를 알면
    /// [`ModTree::resolve_in`]을 쓴다. 대상 하나만 준다 — `#[cfg]`로 갈린 대안 임포트([`Binding::Alternatives`])를
    /// 지나면 우선 순위가 가장 높은 것이다. 간선을 만드는 해석은 대상 전부와 조건을 주는 [`ModTree::resolve_all`]을
    /// 쓴다.
    pub fn resolve(&self, from: &str, segs: &[String], dep_crates: &DepCrates) -> Option<String> {
        first_target(self.resolve_want(from, segs, dep_crates, Want::Any))
    }

    /// [`ModTree::resolve`]와 같되 마지막 세그먼트를 이름공간 `ns`에서 먼저 찾는다 — 값 자리(호출·값 경로)는
    /// [`Ns::Value`], 타입 자리·구조체 리터럴은 [`Ns::Type`], 매크로 호출·속성·derive 인자는 [`Ns::Macro`]다.
    /// 같은 이름이 이름공간마다 다른 정점에 묶였을 때(`fn harvest`와 `use crate::harvest`) 자리에 맞는 쪽을
    /// 고른다. 그 이름공간에 묶인 것이 없으면 [`ModTree::resolve`]처럼 찾는다 — 아이템 표가 모르는 이름공간
    /// (매크로가 만든 아이템·배리언트)에서 해석을 잃지 않는다.
    pub fn resolve_in(
        &self,
        from: &str,
        segs: &[String],
        dep_crates: &DepCrates,
        ns: Ns,
    ) -> Option<String> {
        first_target(self.resolve_want(from, segs, dep_crates, Want::Prefer(ns)))
    }

    /// 경로가 이름 붙이는 정점을 모든 빌드에 걸쳐 전부 준다 — `ns`가 `None`이면 [`ModTree::resolve`], 있으면
    /// [`ModTree::resolve_in`]의 자리다. `#[cfg]`로 갈린 대안 임포트([`Binding::Alternatives`])를 지나면 대안마다
    /// 대상이 하나씩이고, 대상마다 거쳐 온 임포트들의 `#[cfg]`를 합친 조건을 싣는다([`Resolved`]). 간선을 만드는
    /// 해석은 이것을 쓴다 — 대상 하나만 잇으면 다른 빌드의 대상이 거짓 unreachable이 되고, 조건을 버리면
    /// `--target`·`--exclude-tests`가 그 빌드에 없는 임포트를 거친 간선을 지우지 못한다. 첫 원소가
    /// [`ModTree::resolve`]·[`ModTree::resolve_in`]의 결과다. 같은 대상에 여러 경로로 닿으면 하나로 합치고 조건은
    /// 약한 쪽이다(어느 경로로든 닿는다). 해석되지 않으면 비어 있다.
    pub fn resolve_all(
        &self,
        from: &str,
        segs: &[String],
        dep_crates: &DepCrates,
        ns: Option<Ns>,
    ) -> Vec<Resolved> {
        self.resolve_want(from, segs, dep_crates, ns.map_or(Want::Any, Want::Prefer))
    }

    /// 경로 해석의 본체 — `want`는 마지막 세그먼트를 찾는 이름공간이다. 접두사 세그먼트는 늘 타입 이름공간이다.
    /// 대상은 우선 순위 순이다(대안 임포트는 [`Binding::imports`]의 순서로 깊이 우선으로 따른다).
    fn resolve_want(
        &self,
        from: &str,
        segs: &[String],
        dep_crates: &DepCrates,
        want: Want,
    ) -> Vec<Resolved> {
        let Some(first) = segs.first() else {
            return Vec::new();
        };
        let mut i = 1usize;
        let starts: Vec<Resolved> = match first.as_str() {
            "crate" => vec![Resolved::plain(crate_of(from))],
            "self" => vec![Resolved::plain(from.to_string())],
            "super" => {
                let mut m = parent_of(from);
                while i < segs.len() && segs[i] == "super" {
                    m = m.as_deref().and_then(parent_of);
                    i += 1;
                }
                m.map(Resolved::plain).into_iter().collect()
            }
            first => {
                let Some(module) = self.modules.get(from) else {
                    return Vec::new();
                };
                let first_want = if segs.len() > 1 {
                    Want::Prefer(Ns::Type)
                } else {
                    want
                };
                match lookup(module, first, first_want) {
                    Found::Local => vec![Resolved::plain(format!("{from}::{first}"))],
                    Found::LocalOr(imps) => {
                        std::iter::once(Resolved::plain(format!("{from}::{first}")))
                            .chain(imps.iter().map(Resolved::via))
                            .collect()
                    }
                    Found::Import(imps) => imps.iter().map(Resolved::via).collect(),
                    // 그래프 밖(std 등)에서 들인 이름이다 — 크레이트 루트의 같은 이름으로 넘어가지 않는다.
                    Found::Unresolved => return Vec::new(),
                    Found::Unbound => {
                        return self.resolve_unbound(from, first, &segs[1..], dep_crates, want)
                    }
                }
            }
        };
        let mut out = Vec::new();
        for start in starts {
            self.walk(start, &segs[i..], dep_crates, want, &mut out);
        }
        merge_routes(out)
    }

    /// 첫 세그먼트 `first`가 모듈 `from`에 묶이지 않은 경로 — 크레이트 루트의 아이템, 선언된 의존 별칭, 같은
    /// 워크스페이스의 다른 크레이트 루트 순으로 찾는다. `rest`는 나머지 세그먼트다.
    fn resolve_unbound(
        &self,
        from: &str,
        first: &str,
        rest: &[String],
        dep_crates: &DepCrates,
        want: Want,
    ) -> Vec<Resolved> {
        let root = crate_of(from);
        let Some(root_mod) = self.modules.get(&root) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        if root_mod.items.contains(first) || root_mod.children.contains_key(first) {
            let start = Resolved::plain(format!("{root}::{first}"));
            self.walk(start, rest, dep_crates, want, &mut out);
        } else if let Some(t) = dep_crates.lookup(from, first) {
            // 선언된 의존 별칭이 워크스페이스 루트보다 우선한다 —
            // 같은 이름의 멤버가 있어도 `app`의 `foo`는 선언된
            // 패키지다. 외부는 크레이트 정점으로 붕괴하고 멤버
            // 별칭은 그 멤버의 루트 정점으로 재작성해 계속 걷는다
            // — 패키지 이름과 타깃(루트) 이름이 다를 수 있다.
            if t.member {
                self.walk(
                    Resolved::plain(t.vertex.clone()),
                    rest,
                    dep_crates,
                    want,
                    &mut out,
                );
            }
            // 루트 모듈이 트리에 없는 멤버(proc-macro 크레이트 등)는 내부를 모른다 — 외부처럼 크레이트 정점으로
            // 붕괴한다. 안 그러면 `use member_lib;`·`use member::x`가 미해석이 돼 dep 사용 증거가 새 나간다.
            if !t.member || (out.is_empty() && !self.modules.contains_key(&t.vertex)) {
                out.push(Resolved::plain(t.vertex.clone()));
            }
        } else if self.modules.contains_key(first) {
            // 같은 워크스페이스의 다른 크레이트 루트 모듈 —
            // 선언 없는 직접 참조는 컴파일되지 않지만, 관대하게
            // 해석하는 쪽이 잃는 것보다 낫다.
            self.walk(
                Resolved::plain(first.to_string()),
                rest,
                dep_crates,
                want,
                &mut out,
            );
        }
        merge_routes(out)
    }

    /// 경로 `route`에서 나머지 세그먼트 `rest`를 따라 걸어 닿은 대상을 `out`에 우선 순위 순으로 더한다.
    /// 중간 세그먼트가 모듈이면 내려가고, 아니면 아이템 경로로 존재를 확인한다.
    /// 지나는 모듈이 그 세그먼트를 선언하지 않고 `use`로 들였으면(재노출 — `pub use inner::Tr`,
    /// `pub use inner::*`가 펼친 이름, 모듈 별칭) 그 임포트의 대상에서 이어 걷는다 — 정규 ID는
    /// 정의 위치다. 따라갈 때마다 세그먼트를 하나 소비하므로 재노출이 서로를 가리켜도 끝난다.
    /// 그 임포트가 `#[cfg]`로 갈린 대안이면 대안마다 걷고(깊이 우선 — 첫 대안의 대상이 먼저다), 따라간 임포트의
    /// 조건을 경로에 더한다. 대상이 외부 크레이트 정점이면 그 정점으로 붕괴한다([`ModTree::resolve`]와 같다).
    /// 중간 세그먼트는 타입 이름공간에서, 마지막 세그먼트는 `want`로 찾는다 — 로컬 아이템이 먼저인 것은
    /// 첫 세그먼트와 같은 규칙이다([`lookup`]). 비공개 `use`도 따른다 — 크레이트 안에서는 자손 모듈이 볼 수
    /// 있고, 보이지 않는 경로는 컴파일되지 않으니 관대하게 해석해 잃는 것이 없다.
    fn walk(
        &self,
        route: Resolved,
        rest: &[String],
        dep_crates: &DepCrates,
        want: Want,
        out: &mut Vec<Resolved>,
    ) {
        // `use serde::Deserialize`가 만든 임포트처럼 지금 닿은 것이 외부 크레이트 정점이면 그 정점이다 — 외부 안은
        // 못 본다. 멤버 정점은 external에 없다 — 멤버 안은 실제 정점으로 걷는다.
        if dep_crates.external.contains(route.target.as_str()) {
            out.push(route);
            return;
        }
        let Some((seg, tail)) = rest.split_first() else {
            if self.modules.contains_key(&route.target) || self.item_exists(&route.target) {
                out.push(route);
            }
            return;
        };
        let here = if tail.is_empty() {
            want
        } else {
            Want::Prefer(Ns::Type)
        };
        let found = self
            .modules
            .get(&route.target)
            .map_or(Found::Unbound, |m| lookup(m, seg, here));
        match found {
            Found::Import(imps) => {
                for imp in imps {
                    self.walk(route.through(imp), tail, dep_crates, want, out);
                }
            }
            // `#[cfg]`를 단 로컬 아이템이 먼저고, 그것이 없는 빌드의 임포트가 대안이다.
            Found::LocalOr(imps) => {
                self.walk_declared(route.clone(), rest, dep_crates, want, out);
                for imp in imps {
                    self.walk(route.through(imp), tail, dep_crates, want, out);
                }
            }
            Found::Local | Found::Unresolved | Found::Unbound => {
                self.walk_declared(route, rest, dep_crates, want, out)
            }
        }
    }

    /// [`ModTree::walk`]에서 다음 세그먼트(`rest`의 첫 원소)를 지금 닿은 모듈의 선언으로 따라간다 — 자식 모듈이면
    /// 내려가고, 아니면 나머지 전부가 아이템 하위 경로인지 확인한다.
    fn walk_declared(
        &self,
        route: Resolved,
        rest: &[String],
        dep_crates: &DepCrates,
        want: Want,
        out: &mut Vec<Resolved>,
    ) {
        let Some((seg, tail)) = rest.split_first() else {
            return;
        };
        let child = format!("{}::{seg}", route.target);
        if self.modules.contains_key(&child) {
            return self.walk(route.at(child), tail, dep_crates, want, out);
        }
        // 모듈이 아니면 아이템이다 — 뒤는 전부 아이템 하위 경로.
        let candidate = format!("{}::{}", route.target, rest.join("::"));
        if self.item_exists(&candidate) {
            out.push(route.at(candidate));
        }
    }

    /// `use` 경로 하나를 이름공간마다 해석한다 — 명시 `use a::x`는 `a`에서 `x`가 차지한 이름공간을 모두 들인다
    /// (`a`에 `fn x`와 `pub use crate::m as x`가 함께 있으면 값은 그 fn, 타입은 `m`). 앞 세그먼트는 타입
    /// 이름공간으로 걷고, 마지막 세그먼트를 그 모듈에서 이름공간마다 찾는다. 대상이 `None`이면 그 이름공간에서
    /// 그래프 밖 이름으로 묶였다는 뜻이다. 그 모듈에서 이름공간을 정하지 못한 칸([`PerNs::undetermined`])은 그대로
    /// 이름공간을 정하지 못한 칸으로 물려받는다. 어디서도 정점을 정하지 못하면 이름공간을 가리지 않는
    /// 해석([`ModTree::resolve_all`])을 이름공간을 정하지 못한 칸에 둔다 — 외부 크레이트로 붕괴한 경로, 멤버 루트, 아이템
    /// 표가 이름공간을 모르는 이름이 그렇다. 그래서 이름공간을 나누기 전에 묶이던 대상은 늘 어느 칸엔가 있다.
    ///
    /// 앞 세그먼트나 그 모듈의 임포트가 `#[cfg]`로 갈린 대안이면 대안마다 찾고, 칸에는 찾은 대상을 우선 순위
    /// 순으로 둔다. 대상마다 거쳐 온 재노출의 조건을 싣는다 — `use crate::sync::Condvar`가 `sync`의
    /// `#[cfg(feature = "pl")] pub use crate::pl::Condvar`를 거치면 `feature = "pl"`이다. 비어 있는 칸은 `None`이다.
    fn resolve_use(
        &self,
        from: &str,
        segs: &[String],
        dep_crates: &DepCrates,
    ) -> PerNs<Vec<UseTarget>> {
        let mut out: PerNs<Vec<UseTarget>> = PerNs::default();
        if let Some((last, prefix)) = segs.split_last().filter(|(_, p)| !p.is_empty()) {
            for owner in self.resolve_want(from, prefix, dep_crates, Want::Prefer(Ns::Type)) {
                if dep_crates.external.contains(owner.target.as_str()) {
                    continue;
                }
                let Some(module) = self.modules.get(&owner.target) else {
                    continue;
                };
                for ns in Ns::ALL {
                    let local = || UseTarget {
                        target: Some(format!("{}::{last}", owner.target)),
                        cfg: owner.cfg.clone(),
                    };
                    let found = match strict_lookup(module, last, ns) {
                        Found::Local => vec![local()],
                        Found::LocalOr(imps) => std::iter::once(local())
                            .chain(self.use_targets(Found::Import(imps), &owner, dep_crates))
                            .collect(),
                        found => self.use_targets(found, &owner, dep_crates),
                    };
                    extend_slot(out.slot(ns), found);
                }
                if let Some(binding) = module
                    .imports
                    .get(last)
                    .and_then(|b| b.undetermined.as_ref())
                {
                    let found = self.use_targets(found_of(binding), &owner, dep_crates);
                    extend_slot(&mut out.undetermined, found);
                }
            }
        }
        if out.bound().flatten().any(|t| t.target.is_some()) {
            return out;
        }
        let all: Vec<UseTarget> = self
            .resolve_want(from, segs, dep_crates, Want::Any)
            .into_iter()
            .map(|r| UseTarget {
                target: Some(r.target),
                cfg: r.cfg,
            })
            .collect();
        if all.is_empty() {
            out
        } else {
            PerNs::undetermined_only(all)
        }
    }

    /// [`ModTree::resolve_use`]가 대상 모듈(경로 `owner`로 닿은)에서 찾은 임포트 칸을 대상들로 — 대상이 `None`이면
    /// 그래프 밖 이름이다. 조건은 `owner`까지 거쳐 온 것과 그 임포트의 것을 함께 싣는다. 따라간 대상이 정점이
    /// 아니면(트리에 없는 멤버 루트 등) 그 대상은 빠진다.
    fn use_targets(
        &self,
        found: Found<'_>,
        owner: &Resolved,
        dep_crates: &DepCrates,
    ) -> Vec<UseTarget> {
        match found {
            Found::Import(imps) => imps
                .iter()
                .filter(|imp| {
                    dep_crates.external.contains(imp.target.as_str())
                        || self.modules.contains_key(&imp.target)
                        || self.item_exists(&imp.target)
                })
                .map(|imp| UseTarget {
                    target: Some(imp.target.clone()),
                    cfg: with_import_cfg(&owner.cfg, &imp.cfg),
                })
                .collect(),
            Found::Unresolved => vec![UseTarget {
                target: None,
                cfg: owner.cfg.clone(),
            }],
            // 로컬 아이템은 호출자가 정규 ID로 바꾼다 — 여기 오는 것은 임포트 칸뿐이다.
            Found::Local | Found::LocalOr(_) | Found::Unbound => Vec::new(),
        }
    }

    /// 정점 `path`(모듈, 또는 모듈의 선언 아이템)가 모듈 `from`에서 보이는가 — 그 아이템이 선언한 가시성만 본다
    /// (경로가 지나는 모듈의 가시성과 재노출은 보지 않는다). 아이템 표에 없는 정점(크레이트 루트, 외부 크레이트
    /// 정점)은 보인다고 본다. 확정 해석은 컴파일되는 코드를 전제로 관대하게 하므로 이것을 쓰지 않는다 — 대신
    /// 해석하지 않은 후보(추정 간선)가 그 자리에서 쓸 수 없는 이름인지 거르는 데 쓴다.
    pub fn visible_from(&self, path: &str, from: &str) -> bool {
        let Some((owner, name)) = path.rsplit_once("::") else {
            return true;
        };
        self.modules
            .get(owner)
            .and_then(|m| m.item_scopes.get(name))
            .is_none_or(|scope| scope.includes(from))
    }

    /// 경로의 마지막 세그먼트가 어떤 모듈의 선언 아이템인지 확인한다.
    /// 아이템 표는 전 모듈에 대해 먼저 채워지므로 이 판정은 순서 무관하다.
    pub fn item_exists(&self, path: &str) -> bool {
        let Some((owner, name)) = path.rsplit_once("::") else {
            return false;
        };
        // 메서드 경로(Type::method)는 아이템 표에 없으므로 모듈 아이템만 본다.
        self.modules
            .get(owner)
            .is_some_and(|m| m.items.contains(name))
    }
}

/// 우선 순위 순의 대상들 가운데 첫 번째의 정점 ID — 대상 하나를 고르는 해석([`ModTree::resolve`])의 결과다.
fn first_target(routes: Vec<Resolved>) -> Option<String> {
    routes.into_iter().next().map(|r| r.target)
}

/// 우선 순위 순의 대상들에서 같은 정점을 하나로 합친다 — 자리는 처음 나온 곳이고, 조건은 둘 중 약한 쪽이다
/// (어느 경로로든 그 정점에 닿으면 그 이름이 가리키는 것이다).
fn merge_routes(routes: Vec<Resolved>) -> Vec<Resolved> {
    let mut out: Vec<Resolved> = Vec::with_capacity(routes.len());
    for route in routes {
        match out.iter_mut().find(|seen| seen.target == route.target) {
            Some(seen) => seen.cfg = weaker_cfg(&seen.cfg, &route.cfg),
            None => out.push(route),
        }
    }
    out
}

/// [`ModTree::resolve_use`]의 칸 하나에 찾은 대상들을 더한다 — 찾은 것이 없으면 칸을 만들지 않는다(비어 있는
/// 칸은 `None`이라는 불변식).
fn extend_slot(slot: &mut Option<Vec<UseTarget>>, found: Vec<UseTarget>) {
    if !found.is_empty() {
        slot.get_or_insert_with(Vec::new).extend(found);
    }
}

/// 조건 `own`(간선·경로의 것)에 거쳐 온 임포트의 조건 `via`를 더한다 — 둘 다 성립해야 한다. `via`가 없거나 그
/// 조건들이 이미 `own`에 있으면 `own`을 글자 그대로 둔다(아이템과 임포트가 같은 `#[cfg(feature = "x")]`을 달면 간선의
/// 표기가 바뀌지 않는다). `own`이 없으면 `via` 그대로, 둘 다 있으면 [`conjoin_cfg`]처럼 정렬된 조건 집합의
/// `all(..)`이다.
pub fn with_import_cfg(own: &Option<String>, via: &Option<String>) -> Option<String> {
    let Some(via_cfg) = via else {
        return own.clone();
    };
    let Some(own_cfg) = own else {
        return via.clone();
    };
    let (mut have, mut add) = (BTreeSet::new(), BTreeSet::new());
    push_conjuncts(own_cfg.trim(), &mut have);
    push_conjuncts(via_cfg.trim(), &mut add);
    if add.is_subset(&have) {
        own.clone()
    } else {
        conjoin_cfg(own, via)
    }
}

/// 앞선 [`fill_items`]가 이름공간 `ns`에 `name`을 선언 아이템으로 적었는가 — 자식 모듈 표는 보지 않는다.
fn declared_in(module: &Module, name: &str, ns: Ns) -> bool {
    match ns {
        Ns::Type => module.type_items.contains(name),
        Ns::Value => module.value_items.contains(name),
        Ns::Macro => module.macro_items.contains(name),
    }
}

/// 모듈 `module`의 로컬 아이템 가운데 이름공간 `ns`에 `name`이 있는가 — 자식 모듈은 타입 이름공간이다.
fn local_in(module: &Module, name: &str, ns: Ns) -> bool {
    match ns {
        Ns::Type => module.children.contains_key(name) || module.type_items.contains(name),
        Ns::Value => module.value_items.contains(name),
        Ns::Macro => module.macro_items.contains(name),
    }
}

/// 모듈 `module`에서 `name`을 이름공간 `ns`에서만 찾는다 — 그 이름공간의 로컬 아이템이 같은 이름공간의
/// 임포트(글롭)를 이긴다. 명시 임포트와는 컴파일되는 코드에서 공존하지 못한다. 이름공간을 정하지 못한 칸
/// ([`PerNs::undetermined`])은 보지 않는다.
fn strict_lookup<'a>(module: &'a Module, name: &str, ns: Ns) -> Found<'a> {
    if local_in(module, name, ns) {
        return gated_alternatives(module, name, ns).map_or(Found::Local, Found::LocalOr);
    }
    match module.imports.get(name).and_then(|b| b.get(ns)) {
        Some(binding) => found_of(binding),
        None => Found::Unbound,
    }
}

/// 이름공간 `ns`의 `#[cfg]`를 단 로컬 아이템 `name`이 없는 빌드에서 그 이름이 가리키는 임포트
/// ([`Module::local_alternatives`]) — 없으면 `None`이다.
fn gated_alternatives<'a>(module: &'a Module, name: &str, ns: Ns) -> Option<&'a [Import]> {
    let imports = module.local_alternatives.get(name)?.get(ns)?.imports();
    (!imports.is_empty()).then_some(imports)
}

/// 임포트 칸 하나를 찾은 결과로.
fn found_of(binding: &Binding) -> Found<'_> {
    match binding {
        Binding::Unresolved(_) => Found::Unresolved,
        resolved => Found::Import(resolved.imports()),
    }
}

/// 모듈 `module`에서 `name`을 찾는다 — 무엇을 먼저 보는지는 [`Want`]가 정한다.
///
/// 접두사 자리(`name::item`)는 [`Want::Prefer`]`(Ns::Type)`다. 로컬 타입 이름공간 아이템(자식 모듈·struct·enum·
/// union·trait·type)은 같은 이름의 타입 이름공간 임포트(글롭)를 이기고, 로컬 이름이 값·매크로 아이템뿐이면
/// 타입 이름공간에 묶인 임포트를 따른다 — `fn harvest`가 있는 모듈의 `harvest::f()`는 `use crate::harvest`다.
/// 그 이름공간에 아무것도 없으면 이름공간을 정하지 못한 임포트를 보되, 같은 이름의 로컬 아이템이나 이름공간이
/// 정해진 임포트가 있으면 보지 않는다(매크로 자리는 예외 — [`undetermined_lookup`]). 그래도 없으면 [`Want::Any`]다
/// ([`any_lookup`]).
fn lookup<'a>(module: &'a Module, name: &str, want: Want) -> Found<'a> {
    if let Want::Prefer(ns) = want {
        match strict_lookup(module, name, ns) {
            Found::Unbound => {}
            found => return found,
        }
        match undetermined_lookup(module, name, ns) {
            Found::Unbound => {}
            found => return found,
        }
    }
    any_lookup(module, name)
}

/// 자리의 이름공간에 묶인 것이 없을 때 이름공간을 정하지 못한 임포트([`PerNs::undetermined`])를 쓸 것인가.
///
/// 같은 이름의 로컬 아이템이나 이름공간이 정해진 임포트가 있으면 쓰지 않는다(`Unbound`) — rustc에서 명시 `use`는
/// 같은 이름공간의 로컬 아이템·다른 명시 `use`와 공존하지 못하므로(E0255·E0252), 컴파일되는 코드라면 그 `use`는
/// 그것들과 다른 이름공간에 있다. `pub trait Foo`와 `pub use foo_derive::Foo`(derive 매크로)가 함께 있는 모듈에서
/// 매크로 인자·값 자리의 `Foo`가 그 `use`로 가 trait 간선을 잃던 문제다. 쓰지 않으면 [`any_lookup`]이 로컬
/// 아이템이나 정해진 임포트를 고른다. 아무것도 없으면 그 `use`가 유일한 후보다. `#[cfg]`로 갈린 같은 이름공간의
/// 대안 임포트(워크스페이스 쪽·std 쪽)에는 위 근거가 닿지 않지만 결과는 같다 — 소스 순서와 무관하게 정해진
/// 워크스페이스 쪽이고, 그 이름으로 해석한 간선은 그 임포트의 cfg를 싣는다([`PerNs::undetermined`]).
///
/// 매크로 자리(`ns`가 [`Ns::Macro`])는 다르다 — 여기 왔으면 매크로 이름공간에는 로컬 매크로도 정해진 임포트도 없고,
/// 같은 이름의 로컬 fn·trait·struct·mod나 정해진 타입·값 임포트는 매크로가 될 수 없다. 위 근거대로 그 `use`는 그것들과
/// 다른 이름공간에 있으니 매크로 이름공간에 있을 수 있는 유일한 후보다 — `fn info` 옆 `use log::info`의 `info!()`는
/// 로컬 fn이 아니라 그 `use`다(rustc는 그 fn을 "never used"로 경고한다). 예전에는 로컬 fn이 확정 대상이라 거짓
/// call 간선이 생겼고, 그 fn이 다시 부르는 곳과 거짓 순환이 됐다. 로컬 fn은 자리를 모르는 해석의 대상이라 호출자가
/// 추정 간선으로 남긴다(보존은 그대로다).
fn undetermined_lookup<'a>(module: &'a Module, name: &str, ns: Ns) -> Found<'a> {
    let Some(bindings) = module.imports.get(name) else {
        return Found::Unbound;
    };
    let claimed = ns != Ns::Macro
        && (module.items.contains(name)
            || module.children.contains_key(name)
            || bindings.has_determined());
    match &bindings.undetermined {
        Some(binding) if !claimed => found_of(binding),
        _ => Found::Unbound,
    }
}

/// 자리를 모르는 해석 — 로컬 아이템이 먼저고(매크로가 만든 로컬 아이템은 아이템 표에 없어 이 판단에 들어오지
/// 않는다), 다음은 이름공간이 정해진 해석된 임포트를 값 → 타입 → 매크로 순으로, 이름공간을 정하지 못한 해석된
/// 임포트, 그다음이 해석되지 않는 임포트다. 값이 먼저인 것은 이름공간을 나누기 전 글롭 표가 같은 이름의 선언
/// 아이템(대개 fn)을 고르던 결과를 자리를 모르는 해석에서 유지하기 위해서고, 정해진 것이 먼저인 것은 글롭이
/// 들인 trait(타입 이름공간)이 같은 이름의 derive 재노출(이름공간을 모름)에 가려지지 않게 하기 위해서다.
fn any_lookup<'a>(module: &'a Module, name: &str) -> Found<'a> {
    if module.items.contains(name) || module.children.contains_key(name) {
        // `#[cfg]`를 단 로컬 아이템 옆의 같은 이름공간 임포트는 그 아이템이 없는 빌드의 대안이다.
        let alternatives = [Ns::Value, Ns::Type, Ns::Macro]
            .into_iter()
            .filter(|ns| local_in(module, name, *ns))
            .find_map(|ns| gated_alternatives(module, name, ns));
        return alternatives.map_or(Found::Local, Found::LocalOr);
    }
    let Some(bindings) = module.imports.get(name) else {
        return Found::Unbound;
    };
    let determined = [Ns::Value, Ns::Type, Ns::Macro]
        .into_iter()
        .filter_map(|ns| bindings.get(ns));
    for binding in determined.chain(bindings.undetermined.as_ref()) {
        let imports = binding.imports();
        if !imports.is_empty() {
            return Found::Import(imports);
        }
    }
    if bindings.is_empty() {
        Found::Unbound
    } else {
        Found::Unresolved
    }
}

/// ast 아이템들에서 `mod` 선언을 찾아 트리에 심는다.
/// 반환값: 큐에 넣을 서브모듈 경로들 — 파일·인라인 모두 큐에 넣고,
/// 아이템 목록은 module_items가 파일/인라인을 구분해 찾는다.
/// 선언이 어느 타깃(크레이트 인스턴스)에 속하는지 모르는 호출이라, 순환 판정에서 크레이트
/// 루트의 파일을 모두 조상으로 본다(lib과 같은 이름의 bin이 서로의 루트 파일을 싣는 선언도
/// 순환으로 센다 — 보수적이다). 수확은 타깃을 아는 변형으로 루트 파일 하나만 조상으로 본다.
pub fn collect_submodules(
    items: &[&syn::Item],
    parent_path: &str,
    tree: &mut ModTree,
    conditional_count: &mut usize,
) -> Vec<String> {
    collect_target_submodules(None, items, parent_path, tree, conditional_count)
}

/// [`collect_submodules`]와 같되, 선언이 루트 파일 `target_root`인 타깃 하나의 모듈 트리에
/// 속한다. lib과 같은 이름의 bin은 트리에서 루트 모듈 하나를 함께 쓰지만(file + extra_files)
/// rustc에게는 루트가 다른 두 크레이트다 — bin의 `mod lib;`(lib.rs)와 lib의
/// `#[path = "main.rs"] mod m;`은 순환이 아니라 rustc가 받는 선언이다(special_module_name
/// 경고뿐). 그래서 순환 판정에서 크레이트 루트는 이 타깃의 루트 파일만 조상이다. None이면
/// 루트의 파일을 모두 조상으로 본다.
pub(crate) fn collect_target_submodules(
    target_root: Option<&Path>,
    items: &[&syn::Item],
    parent_path: &str,
    tree: &mut ModTree,
    conditional_count: &mut usize,
) -> Vec<String> {
    // `#[path]` 자식의 기준 디렉터리 — 파일에 직접 선언되면 파일이
    // 놓인 디렉터리(`outer.rs`면 `src/` — module_dir과 다르다),
    // 인라인 모듈 안이면 그 모듈의 실효 디렉터리다(rustc 실증).
    // 일반 자식의 기준은 부모의 실효 디렉터리 `dir`이다 — 인라인
    // 조상의 세그먼트(이름 또는 `#[path]` 오버라이드)가 이미 누적돼
    // 있다.
    let (path_base, child_base, parent_file) = {
        let parent = &tree.modules[parent_path];
        if let Some(root) =
            target_root.filter(|_| !parent_path.contains("::") && parent.file_module)
        {
            // 병합 루트의 대표 파일은 lib일 수 있다. 현재 타깃의 mod 선언은
            // 실제 루트 파일의 디렉터리를 기준으로 읽어야 하며 파일명은 임의여도 된다.
            let base = root.parent().unwrap_or(Path::new(".")).to_path_buf();
            (base.clone(), base, root.to_path_buf())
        } else {
            let path_base = if parent.file_module {
                parent.file.parent().unwrap_or(Path::new(".")).to_path_buf()
            } else {
                parent.dir.clone()
            };
            (path_base, parent.dir.clone(), parent.file.clone())
        }
    };
    // 자식 경로의 세그먼트 수 — 상한을 넘는 선언은 싣지 않고 센다.
    let child_depth = parent_path.split("::").count() + 1;
    let mut queued = Vec::new();
    for item in items {
        let syn::Item::Mod(m) = item else { continue };
        let name = m.ident.to_string();
        let path = format!("{parent_path}::{name}");
        if child_depth > MAX_MODULE_DEPTH {
            tree.skipped_mods.insert(path, SkippedMod::TooDeep);
            continue;
        }
        let cfg = cfg_of(&m.attrs);
        // `#[path = "..."]`는 NameValue 메타다 — `parse_args`는
        // `#[path("...")]` 문법만 받으므로 값은 nv.value에서 읽는다.
        // 인라인 모듈에도 달 수 있고, 그 값은 자식의 기준 디렉터리를
        // 덮어쓴다.
        let path_attr = m
            .attrs
            .iter()
            .find(|a| a.path().is_ident("path"))
            .and_then(|a| match &a.meta {
                syn::Meta::NameValue(nv) => match &nv.value {
                    syn::Expr::Lit(syn::ExprLit {
                        lit: syn::Lit::Str(s),
                        ..
                    }) => Some(s.value()),
                    _ => None,
                },
                _ => None,
            });
        let resolved = if m.content.is_some() {
            // 인라인 모듈 — 같은 파일. `#[path]`는 자식의 기준
            // 디렉터리 세그먼트를 덮어쓴다.
            let dir = match &path_attr {
                Some(p) => path_base.join(p),
                None => child_base.join(name.strip_prefix("r#").unwrap_or(&name)),
            };
            Some((parent_file.clone(), false, dir))
        } else {
            let base = if path_attr.is_some() {
                &path_base
            } else {
                &child_base
            };
            match mod_file(base, &name, path_attr.as_deref()) {
                Some(f) => {
                    if let Some(reason) = file_module_skip(tree, parent_path, &f, target_root) {
                        tree.skipped_mods.insert(path, reason);
                        continue;
                    }
                    // `#[path]`로 로드된 파일은 자기 디렉터리를 소유한다
                    // — `loaded.rs`라도 `loaded/` 스템 디렉터리를
                    // 만들지 않는다(rustc 실증).
                    let dir = if path_attr.is_some() {
                        f.parent().unwrap_or(Path::new(".")).to_path_buf()
                    } else {
                        module_dir(&f)
                    };
                    Some((f, true, dir))
                }
                None => None,
            }
        };
        // 병합 루트(lib+같은 이름 bin)를 두 번째 타깃이 다시 훑을 때 같은
        // mod 선언을 재계수하지 않는다 — 처음 보는 경로일 때만 센다.
        // 건너뛴 선언(skipped_mods)은 실리지 않았으니 세지 않는다.
        if cfg.is_some() && !tree.modules.contains_key(&path) {
            *conditional_count += 1;
        }
        // 파일 없는 mod(조건부·생성) — 정점 없이 limitation만.
        let Some((file, is_file_module, dir)) = resolved else {
            continue;
        };
        let public = matches!(m.vis, syn::Visibility::Public(_));
        let mut module = Module::new(file, is_file_module, public);
        module.cfg = cfg;
        module.dir = dir;
        tree.modules.insert(path.clone(), module);
        tree.modules
            .get_mut(parent_path)
            .expect("parent module must exist")
            .children
            .insert(name, path.clone());
        queued.push(path);
    }
    queued
}

/// 파일 모듈 대상 `file`을 정점으로 싣지 못하는 사유다 — 실을 수 있으면 None.
/// 순환은 조상 사슬만 본다 — 조상이 아닌 두 모듈이 같은 파일을 싣는 `#[path]`는
/// rustc도 받는다. 종류는 stat으로 본다(FIFO에서도 막히지 않는다). stat이 실패하면
/// None이다 — 읽기는 parse_into가 원인을 담은 오류로 낸다(지금과 같다).
fn file_module_skip(
    tree: &ModTree,
    parent_path: &str,
    file: &Path,
    target_root: Option<&Path>,
) -> Option<SkippedMod> {
    if reenters_ancestor(tree, parent_path, file, target_root) {
        return Some(SkippedMod::Circular);
    }
    match std::fs::metadata(file) {
        Ok(md) if !md.is_file() => Some(SkippedMod::NotRegularFile),
        _ => None,
    }
}

/// `file`이 `parent_path`부터 크레이트 루트까지 어느 조상 모듈의 파일인가 — 인라인 조상은
/// 감싼 파일을 file로 가진다. 크레이트 루트는 `target_root`가 있으면 그 파일만, 없으면 병합
/// 루트의 extra_files까지 조상 파일이다. 다른 타깃의 루트 파일을 거쳐 돌아오는 진짜 순환도
/// 그 파일이 루트 아래 모듈의 file로 다시 나타나는 자리에서 끊긴다.
/// 경로는 둘 다 mod_file·grow_tree가 정규화한 것이라 그대로 비교한다.
fn reenters_ancestor(
    tree: &ModTree,
    parent_path: &str,
    file: &Path,
    target_root: Option<&Path>,
) -> bool {
    let mut current = Some(parent_path);
    while let Some(path) = current {
        let up = path.rsplit_once("::").map(|(up, _)| up);
        let ancestor_file = match (up, target_root) {
            (None, Some(root)) => root == file,
            _ => tree
                .modules
                .get(path)
                .is_some_and(|m| m.file == file || m.extra_files.iter().any(|f| f == file)),
        };
        if ancestor_file {
            return true;
        }
        current = up;
    }
    false
}

/// `#[cfg]`가 붙어 있으면 조건부로 본다 — 포함은 하되 실측으로 센다.
pub fn has_cfg(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| a.path().is_ident("cfg"))
}

/// `#[cfg(...)]`의 조건 토큰을 그대로 돌려준다 — `feature = "x"`, `test` 등.
/// 여러 cfg 속성이 붙으면 모두 성립해야 하니 `all(...)`로 합성해 돌린다 —
/// 정점 하나에 조건 하나라는 필드 계약을 유지하기 위해서다.
pub fn cfg_of(attrs: &[syn::Attribute]) -> Option<String> {
    let conds: Vec<String> = attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg"))
        .filter_map(|a| a.meta.require_list().ok().map(|l| l.tokens.to_string()))
        .collect();
    match conds.len() {
        0 => None,
        1 => Some(conds.into_iter().next().expect("len checked")),
        _ => Some(format!("all({})", conds.join(" , "))),
    }
}

/// 모듈의 직접 아이템 이름을 채운다(1단계).
/// `use` 해석은 다른 모듈의 아이템 표에 의존하므로, 전 모듈의 아이템 표가
/// 완성된 뒤 fill_imports가 따로 돌아야 한다 — 같은 패스로 하면 알파벳 순서에
/// 따라 크로스 크레이트 임포트가 조용히 유실된다.
pub fn fill_items(tree: &mut ModTree, path: &str, items: &[&syn::Item]) {
    let mut names = BTreeSet::new();
    let (mut types, mut values, mut macros) = (BTreeSet::new(), BTreeSet::new(), BTreeSet::new());
    let mut scopes: BTreeMap<String, Scope> = BTreeMap::new();
    // (이름, 이름공간) → 이 호출에서 본 그 선언들의 `#[cfg]` 조건 — 조건 없는 선언이 하나라도 있으면 `None`.
    let mut gated: BTreeMap<(String, Ns), Option<Vec<String>>> = BTreeMap::new();
    for item in items {
        // (이름, 가시성, 차지하는 이름공간, 속성). 튜플·유닛 struct는 생성자로 값 이름공간도 차지한다.
        // `macro_rules!`은 경로 가시성이 없어 지금처럼 어디서나 보이는 것으로 둔다.
        let (ident, vis, namespaces, attrs): (_, _, &[Ns], &[syn::Attribute]) = match item {
            syn::Item::Fn(f) => (&f.sig.ident, Some(&f.vis), &[Ns::Value], &f.attrs),
            syn::Item::Struct(s) => match s.fields {
                syn::Fields::Named(_) => (&s.ident, Some(&s.vis), &[Ns::Type], &s.attrs),
                _ => (&s.ident, Some(&s.vis), &[Ns::Type, Ns::Value], &s.attrs),
            },
            syn::Item::Enum(e) => (&e.ident, Some(&e.vis), &[Ns::Type], &e.attrs),
            syn::Item::Trait(t) => (&t.ident, Some(&t.vis), &[Ns::Type], &t.attrs),
            syn::Item::Union(u) => (&u.ident, Some(&u.vis), &[Ns::Type], &u.attrs),
            syn::Item::Type(t) => (&t.ident, Some(&t.vis), &[Ns::Type], &t.attrs),
            syn::Item::Const(c) => (&c.ident, Some(&c.vis), &[Ns::Value], &c.attrs),
            syn::Item::Static(s) => (&s.ident, Some(&s.vis), &[Ns::Value], &s.attrs),
            syn::Item::Mod(m) => (&m.ident, Some(&m.vis), &[Ns::Type], &m.attrs),
            syn::Item::Macro(m) => match &m.ident {
                Some(id) => (id, None, &[Ns::Macro], &m.attrs),
                None => continue,
            },
            _ => continue,
        };
        let condition = cfg_of(attrs);
        for ns in namespaces {
            let seen = gated
                .entry((ident.to_string(), *ns))
                .or_insert_with(|| Some(Vec::new()));
            match (seen.as_mut(), &condition) {
                (Some(conditions), Some(condition)) => conditions.push(condition.clone()),
                _ => *seen = None,
            }
        }
        let name = ident.to_string();
        let scope = vis.map_or(Scope::Public, |vis| scope_of(vis, path));
        let scope = match scopes.get(&name) {
            Some(seen) => seen.wider(&scope),
            None => scope,
        };
        scopes.insert(name.clone(), scope);
        for ns in namespaces {
            match ns {
                Ns::Type => &mut types,
                Ns::Value => &mut values,
                Ns::Macro => &mut macros,
            }
            .insert(name.clone());
        }
        names.insert(name);
    }
    let module = tree.modules.get_mut(path).expect("module must exist");
    // 같은 모듈을 두 번 채우면 앞서 채운 선언과 합친다 — 앞서 본 조건 없는 선언이 있는 이름공간은 조건부가 아니다.
    // 앞선 선언은 아이템 표(이름공간별 표)로 본다 — 자식 모듈 표는 `mod` 선언을 훑을 때 미리 채워지므로, 그것을 보면
    // `#[cfg] mod imp`가 늘 앞선 조건 없는 선언으로 보였다.
    for ((name, ns), conditions) in gated {
        let earlier = declared_in(module, &name, ns);
        let previous = module
            .cfg_gated
            .get(&name)
            .and_then(|spaces| spaces.get(&ns))
            .cloned();
        let merged = match (conditions, earlier, previous) {
            (Some(conditions), false, _) => Some(conditions),
            (Some(conditions), true, Some(previous)) => Some([previous, conditions].concat()),
            _ => None,
        };
        let spaces = module.cfg_gated.entry(name).or_default();
        match merged {
            Some(conditions) => {
                spaces.insert(ns, conditions);
            }
            None => {
                spaces.remove(&ns);
            }
        }
    }
    module.cfg_gated.retain(|_, spaces| !spaces.is_empty());
    module.items.extend(names);
    module.type_items.extend(types);
    module.value_items.extend(values);
    module.macro_items.extend(macros);
    for (name, scope) in scopes {
        let scope = match module.item_scopes.get(&name) {
            Some(seen) => seen.wider(&scope),
            None => scope,
        };
        module.item_scopes.insert(name, scope);
    }
}

/// 선언 모듈 `module`에서 쓴 가시성의 범위. 제한 가시성(`pub(..)`)은 아무리 넓어도 그 크레이트
/// 안이므로, 가리키는 모듈을 찾지 못하면(`super`가 루트를 넘는 등 컴파일되지 않는 코드) 크레이트
/// 전체로 둔다 — 크레이트 밖으로 새지 않는다.
fn scope_of(vis: &syn::Visibility, module: &str) -> Scope {
    match vis {
        syn::Visibility::Public(_) => Scope::Public,
        syn::Visibility::Inherited => Scope::Within(module.to_string()),
        syn::Visibility::Restricted(r) => {
            let segs: Vec<String> = r
                .path
                .segments
                .iter()
                .map(|s| s.ident.to_string())
                .collect();
            Scope::Within(restricted_scope(&segs, module).unwrap_or_else(|| crate_of(module)))
        }
    }
}

/// `pub(crate)`·`pub(self)`·`pub(super)`·`pub(in crate::a)`의 경로를 모듈 경로로. `crate`·`self`·
/// `super`로 시작하지 않는 `pub(in top::a)`는 2015 판의 크레이트 기준 경로다(2018 판부터는 쓸 수 없다).
fn restricted_scope(segs: &[String], module: &str) -> Option<String> {
    let (first, rest) = segs.split_first()?;
    let mut cur = match first.as_str() {
        "crate" => crate_of(module),
        "self" => module.to_string(),
        "super" => parent_of(module)?,
        other => format!("{}::{other}", crate_of(module)),
    };
    for seg in rest {
        cur = if seg == "super" {
            parent_of(&cur)?
        } else {
            format!("{cur}::{seg}")
        };
    }
    Some(cur)
}

/// `#[proc_macro_derive(Name, ..)]`를 단 함수가 등록한 derive 이름 → 그 함수를 가리키는 공개 임포트.
///
/// 등록 이름은 매크로 이름공간에 묶인다 — 같은 모듈에 같은 이름의 struct가 있어도(proc-macro 크레이트는
/// 매크로 말고는 내보낼 수 없어 그런 struct는 크레이트 밖에서 보이지 않는다) 서로 가리지 않는다. 인자를
/// 파싱하지 못한 속성은 건너뛴다(이름을 지어내지 않는다).
fn derive_macro_names(path: &str, items: &[&syn::Item]) -> Vec<(String, Import)> {
    let mut names = Vec::new();
    for item in items {
        let syn::Item::Fn(f) = item else {
            continue;
        };
        for attr in &f.attrs {
            if !attr.path().is_ident("proc_macro_derive") {
                continue;
            }
            if let Some(name) = derive_name(attr) {
                names.push((
                    name,
                    Import {
                        target: format!("{path}::{}", f.sig.ident),
                        cfg: cfg_of(&f.attrs),
                        scope: Scope::Public,
                    },
                ));
            }
        }
    }
    names
}

/// `proc_macro_derive(Name, attributes(..))`의 첫 인자 `Name`.
fn derive_name(attr: &syn::Attribute) -> Option<String> {
    let args = attr
        .parse_args_with(syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated)
        .ok()?;
    match args.first()? {
        syn::Meta::Path(p) => p.get_ident().map(|id| id.to_string()),
        _ => None,
    }
}

/// 모듈의 `use` 임포트 맵을 해석해 채운다(2단계 — 전 모듈의 fill_items 이후).
///
/// 이 모듈의 임포트 표([`Module::imports`])는 부를 때마다 그 모듈의 `use`로 새로 만든다 — 해석은 다른 모듈과
/// 이 모듈의 지난 표를 읽고, 다 만든 표로 바꾼다. 그래서 글롭 대상이 바뀌면 지난번에 펼친 이름이 남지 않는다.
pub fn fill_imports(tree: &mut ModTree, path: &str, items: &[&syn::Item], dep_crates: &DepCrates) {
    fill_imports_from(tree, None, path, items, dep_crates);
}

/// 모듈 하나의 임포트 표와 글롭 대상 — [`fill_imports_from`]이 바꾸기 전 표를 돌려준다.
type Tables = (
    BTreeMap<String, PerNs<Binding>>,
    BTreeMap<String, Option<String>>,
);

/// [`fill_imports`]의 본체 — `frozen`이 있으면 명시 `use`와 글롭 대상 경로를 그 트리(앞서 수렴한 임포트 표)로
/// 해석하고, 글롭이 펼치는 이름만 `tree`의 지금 표에서 읽는다([`fill_imports_settled`]의 깨끗한 바퀴).
fn fill_imports_from(
    tree: &mut ModTree,
    frozen: Option<&ModTree>,
    path: &str,
    items: &[&syn::Item],
    dep_crates: &DepCrates,
) -> Tables {
    let resolver: &ModTree = frozen.unwrap_or(tree);
    let derive_names = derive_macro_names(path, items);
    let mut uses: Vec<(Vec<String>, Option<String>, Scope)> = Vec::new();
    for item in items {
        if let syn::Item::Use(u) = item {
            let cfg = cfg_of(&u.attrs);
            let scope = scope_of(&u.vis, path);
            flatten_use(&u.tree, &mut Vec::new(), &mut |segs| {
                uses.push((segs, cfg.clone(), scope.clone()));
            });
        }
    }
    // 해석은 불변 참조가 필요하니 먼저 모으고 그 다음 심는다.
    // 글롭(`use m::*`)은 대상 모듈의 아이템과 자식 모듈, 그리고 그 모듈이 `use`로 들인 이름(재노출)
    // 가운데 이 모듈에서 보이는 것만 임포트한다(rustc와 같다) — `m` 자체만 매핑하면 `f()` 호출이
    // 미해석으로 새니 아이템별로 임포트 맵을 펼친다. 이름공간마다 따로 들이고, 명시 임포트는 해석되지 않는
    // std 경로여도 같은 이름공간의 글롭 임포트를 소스 순서와 무관하게 가린다.
    let mut globbed: BTreeMap<String, PerNs<Binding>> = BTreeMap::new();
    let mut explicit: BTreeMap<String, PerNs<Binding>> = BTreeMap::new();
    // 칸마다 그 칸을 차지한 명시 `use`들의 자기 `#[cfg]`(없으면 `None`) — 같은 칸의 글롭을 얼마나 가리는지 정한다
    // ([`shadow_globs`]).
    let mut conditions: BTreeMap<String, PerNs<Vec<Option<String>>>> = BTreeMap::new();
    let mut glob_targets: BTreeMap<String, Option<String>> = BTreeMap::new();
    for (segs, cfg, scope) in &uses {
        let glob = segs.last().is_some_and(|s| s == "*");
        let clean: Vec<String> = segs
            .iter()
            .filter(|s| !s.starts_with("as ") && *s != "*")
            .cloned()
            .collect();
        if glob {
            // 해석되지 않는 글롭(외부 크레이트)은 무슨 이름을 들이는지 모른다 — 가릴 것도 없다. 글롭 대상은 모듈·
            // 열거형이라 타입 이름공간에서 찾는다 — `fn harvest`와 `use crate::harvest`가 함께 있는 모듈의
            // `use super::harvest::*`가 그 fn을 대상으로 잡아 아무것도 들이지 않던 문제다.
            // 대상 경로가 `#[cfg]`로 갈린 대안 임포트를 지나면 대안마다 들인다(조건은 글롭의 것과 거쳐 온 임포트의
            // 것을 함께). 우선 순위가 낮은 대안부터 심어, 대상 하나를 고르는 해석의 대상이 칸의 첫 원소가 된다.
            for full in resolver
                .resolve_all(path, &clean, dep_crates, Some(Ns::Type))
                .into_iter()
                .rev()
            {
                let glob_cfg = with_import_cfg(cfg, &full.cfg);
                if let Some(target) = tree.modules.get(&full.target) {
                    for (name, bindings) in
                        glob_bindings(target, &full.target, path, &glob_cfg, scope)
                    {
                        merge_glob(globbed.entry(name).or_default(), bindings);
                    }
                }
                // 글롭한 모듈 자신은 이름으로 묶지 않는다(`use m::*`는 `m`을 들이지 않는다) — 그 글롭의 의존
                // 증거(대상 모듈·외부 크레이트 정점으로 가는 uses 간선)로만 따로 둔다. 같은 대상을 여러 글롭이
                // 들이면 약한 조건이다.
                let glob_cfg = match glob_targets.remove(&full.target) {
                    Some(seen) => weaker_cfg(&seen, &glob_cfg),
                    None => glob_cfg,
                };
                glob_targets.insert(full.target, glob_cfg);
            }
            continue;
        }
        let alias = segs
            .iter()
            .rev()
            .find(|s| s.starts_with("as "))
            .map(|s| s[3..].to_string())
            .unwrap_or_else(|| segs.last().unwrap().clone());
        let targets = resolver.resolve_use(path, &clean, dep_crates);
        // 어디에도 묶이지 않는 `use`(std 경로 등)는 이름공간을 정하지 못한, 해석되지 않는 이름이다.
        let targets = if targets.is_empty() {
            PerNs::undetermined_only(vec![UseTarget {
                target: None,
                cfg: None,
            }])
        } else {
            targets
        };
        let own = conditions.entry(alias.clone()).or_default();
        let slots = explicit.entry(alias).or_default();
        // 같은 이름공간에 같은 이름을 들이는 `use`가 둘이면 대상이 다를 때 둘 다 남는다([`merge_slot`] —
        // `#[cfg]`로 갈린 대안이다). 대상 하나를 고르는 해석은 지금처럼 뒤의 것을 고른다.
        for ns in Ns::ALL {
            if let Some(found) = targets.get(ns) {
                merge_slot(slots.slot(ns), use_binding(found, cfg, scope));
                own.slot(ns).get_or_insert_with(Vec::new).push(cfg.clone());
            }
        }
        if let Some(found) = &targets.undetermined {
            merge_slot(&mut slots.undetermined, use_binding(found, cfg, scope));
            own.undetermined
                .get_or_insert_with(Vec::new)
                .push(cfg.clone());
        }
    }
    // derive 매크로는 함수 이름이 아니라 등록한 이름으로 쓰인다 — `pub use self::hello_impl as Hello;`
    // 재노출처럼 그 이름을 매크로 이름공간에서 함수 정점으로 잇는다. 없으면 `#[derive(pm::Hello)]`가 미해석이라
    // deps가 실제로 쓰는 proc-macro 의존을 unused로 보고했다. 명시 `use`가 같은 이름을 매크로 이름공간에
    // 들이면 그쪽이 이긴다.
    for (name, import) in derive_names {
        conditions
            .entry(name.clone())
            .or_default()
            .macros
            .get_or_insert_with(Vec::new)
            .push(import.cfg.clone());
        explicit
            .entry(name)
            .or_default()
            .macros
            .get_or_insert(Binding::Resolved(import));
    }
    let local_alternatives = match tree.modules.get(path) {
        Some(module) => local_alternatives(tree, module, path, &explicit, &conditions, &globbed),
        None => BTreeMap::new(),
    };
    let mut imports = globbed;
    for (name, bindings) in explicit {
        // 이름을 묶지 않는 `use T as _`는 아무것도 가리지 않는다 — 글롭이 들인 `_`와 함께 남는다.
        let own = match conditions.remove(&name) {
            Some(own) if name != "_" => own,
            _ => PerNs::default(),
        };
        shadow_globs(imports.entry(name).or_default(), bindings, &own);
    }
    let module = tree.modules.get_mut(path).expect("module must exist");
    module.local_alternatives = local_alternatives;
    (
        std::mem::replace(&mut module.imports, imports),
        std::mem::replace(&mut module.glob_targets, glob_targets),
    )
}

/// 명시 `use` 하나(조건 `cfg`, 가시성 `scope`)가 한 이름공간에서 찾은 대상들의 칸 값 — 정점으로 해석된 대상이
/// 있으면 그것들이고(조건은 그 `use`의 것과 거쳐 온 재노출의 것을 함께), 없으면 그래프 밖 이름이다.
fn use_binding(found: &[UseTarget], cfg: &Option<String>, scope: &Scope) -> Binding {
    let imports = found
        .iter()
        .filter_map(|t| {
            t.target.as_ref().map(|target| Import {
                target: target.clone(),
                cfg: with_import_cfg(cfg, &t.cfg),
                scope: scope.clone(),
            })
        })
        .collect();
    Binding::of_imports(merge_imports(imports))
        .unwrap_or_else(|| Binding::Unresolved(scope.clone()))
}

/// 한 이름의 글롭 칸 `slots` 위에 명시 임포트 `explicit`을 얹는다 — 명시 임포트는 자기 이름공간의 글롭만 가린다.
/// 이름공간을 정하지 못한 명시 임포트는 어느 이름공간을 차지하는지 모르니 그 이름의 글롭을 모두 가린다 —
/// `use std::fmt::Result`가 글롭이 들인 로컬 `Result`를 가리는 것(이름공간을 나누기 전과 같다).
///
/// `own`은 칸마다 그 칸을 차지한 명시 `use`들의 자기 `#[cfg]`다. 조건 없는 명시 `use`가 있으면 그 칸의 글롭을 다
/// 가린다. 모두 자기 `#[cfg]`를 달았으면 그 조건이 아닌 빌드에서는 글롭의 이름이 보이므로 글롭 칸과 합친다
/// ([`merge_slot`] — 명시 임포트가 첫 원소) — 다만 글롭 임포트의 조건이 명시 `use`의 조건을 함축하면(글롭이 보이는
/// 빌드에서는 늘 명시 `use`도 있으면) 그 글롭 임포트는 가린다(`#[cfg(feature = "x")] use a::*;` 옆
/// `#[cfg(feature = "x")] use b::f;`의 `a::f`). `use a::*;` 옆 `#[cfg(test)] use a::f;`는 같은 대상이라 약한
/// 조건(조건 없음)으로 합쳐지고, `use real::*;` 옆 `#[cfg(test)] use mock::Clock;`은 두 대안으로 남는다. 예전에는 늘
/// 가려, 테스트가 아닌 빌드에서 쓰이는 글롭 쪽 이름에 `cfg(test)`가 붙거나 간선이 아예 없었다. 남는 글롭 쪽은 자기
/// 조건을 그대로 싣는다(명시 `use` 조건의 부정을 더하지 않는다 — 간선을 남기는 쪽이다).
fn shadow_globs(
    slots: &mut PerNs<Binding>,
    explicit: PerNs<Binding>,
    own: &PerNs<Vec<Option<String>>>,
) {
    let PerNs {
        types,
        values,
        macros,
        undetermined,
    } = explicit;
    // 조건을 모르는 칸(`_` — `own`에 없다)은 아무것도 가리지 않는다. 조건 없는 명시 `use`가 있으면 다 가린다.
    if let Some(binding) = undetermined {
        match own.undetermined.as_deref() {
            Some(conditions) if conditions.iter().any(Option::is_none) => {
                *slots = PerNs {
                    undetermined: Some(binding),
                    ..PerNs::default()
                };
            }
            _ => merge_slot(&mut slots.undetermined, binding),
        }
    }
    for (slot, binding, conditions) in [
        (&mut slots.types, types, own.types.as_deref()),
        (&mut slots.values, values, own.values.as_deref()),
        (&mut slots.macros, macros, own.macros.as_deref()),
    ] {
        let Some(binding) = binding else {
            continue;
        };
        let conditional: Option<Vec<&str>> = conditions
            .unwrap_or_default()
            .iter()
            .map(Option::as_deref)
            .collect();
        match conditional {
            Some(conditions) => {
                *slot = slot.take().and_then(|glob| unshadowed(glob, &conditions));
                merge_slot(slot, binding);
            }
            None => *slot = Some(binding),
        }
    }
}

/// 글롭 칸 `glob`에서 조건이 `conditions`(명시 `use`들의 자기 조건) 가운데 하나를 함축하는 임포트를 뺀 것 — 그
/// 글롭 임포트가 보이는 빌드에는 늘 그 명시 `use`도 있어 가려진다. 해석되지 않는 이름은 조건을 모르니 그대로 둔다.
fn unshadowed(glob: Binding, conditions: &[&str]) -> Option<Binding> {
    if glob.imports().is_empty() {
        return Some(glob);
    }
    let kept = glob
        .imports()
        .iter()
        .filter(|imp| !conditions.iter().any(|c| implies(&imp.cfg, c)))
        .cloned()
        .collect();
    Binding::of_imports(kept)
}

/// 조건 `cfg`가 조건 `condition`을 함축하는가 — `condition`의 조건들(맨 위 `all(..)`을 펼친 것)이 모두 `cfg`에 있다.
fn implies(cfg: &Option<String>, condition: &str) -> bool {
    let Some(cfg) = cfg else {
        return false;
    };
    let (mut have, mut need) = (BTreeSet::new(), BTreeSet::new());
    push_conjuncts(cfg.trim(), &mut have);
    push_conjuncts(condition.trim(), &mut need);
    !need.is_empty() && need.is_subset(&have)
}

/// 정점 `target`이 스스로 `#[cfg]`를 단 것인가 — `#[cfg]`를 단 `mod`이거나, 선언한 모듈에서 모든 선언이 `#[cfg]`를 단
/// 아이템([`Module::cfg_gated`])이다.
fn gated_vertex(tree: &ModTree, target: &str) -> bool {
    if tree.modules.get(target).is_some_and(|m| m.cfg.is_some()) {
        return true;
    }
    target.rsplit_once("::").is_some_and(|(owner, name)| {
        tree.modules
            .get(owner)
            .is_some_and(|m| m.cfg_gated.contains_key(name))
    })
}

/// 조건들이 함께 성립할 수 없음이 글자로 보이는가 — 어떤 조건 `not (Q)`의 Q를 맨 위 `all(..)`로 펼친 조건이 모두
/// 다른 조건들 가운데 있다(`unix`와 `not (unix)`, `test`·`loom`과 `not (all (test , loom))`). 공백은 무시한다. 그 밖의
/// 모순(`unix`와 `windows`, `any`의 부정)은 보지 않는다 — 이것으로 지우는 간선은 어느 빌드에도 없는 것뿐이다.
pub fn cfg_contradicts<'a>(conditions: impl IntoIterator<Item = &'a str>) -> bool {
    let squash = |c: &str| {
        c.chars()
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>()
    };
    let mut all = BTreeSet::new();
    for condition in conditions {
        push_conjuncts(&squash(condition), &mut all);
    }
    all.iter().any(|conjunct| {
        let Some(inner) = call_arguments(conjunct, "not") else {
            return false;
        };
        let mut need = BTreeSet::new();
        push_conjuncts(inner, &mut need);
        !need.is_empty() && need.is_subset(&all)
    })
}

/// 모듈 `path`(=`module`)에서 `#[cfg]`를 단 로컬 아이템([`Module::cfg_gated`])이 없는 빌드의 대안 임포트를 고른다
/// ([`Module::local_alternatives`]) — 같은 이름공간의 명시 `use`(`explicit`)는 늘, 글롭 이름(`globbed`)은 아이템의
/// 선언이 하나이고 글롭의 대상이 스스로 `#[cfg]`를 달지 않았을 때만 그 조건의 부정을 달고 대안이다. 그 글롭이 보이는
/// 빌드에 늘 아이템이 있거나(글롭의 조건이 아이템의 조건을 함축 — 같은 `#[cfg(feature = "x")]`), 아이템이 없는
/// 빌드에서 같은 칸의 명시 `use`(자기 조건은 `conditions_of`)가 그 글롭을 가리면(`#[cfg(not(unix))] use crate::b::sys;`
/// 옆 `#[cfg(unix)] fn sys`) 그 글롭은 대안이 아니다 — 어느 빌드에서도 보이지 않는 이름으로 가는 거짓 간선이 된다.
/// 대상이 조건부면
/// (`#[cfg(any(test, kani))] mod tests` 옆 `use crate::*`가 들인 루트의 `#[cfg(test)] mod tests`) 그 조건이 부정과
/// 겹치는지 알 수 없어 대안으로 두지 않는다(거짓 간선이 될 수 있다).
fn local_alternatives(
    tree: &ModTree,
    module: &Module,
    path: &str,
    explicit: &BTreeMap<String, PerNs<Binding>>,
    conditions_of: &BTreeMap<String, PerNs<Vec<Option<String>>>>,
    globbed: &BTreeMap<String, PerNs<Binding>>,
) -> BTreeMap<String, PerNs<Binding>> {
    let mut out: BTreeMap<String, PerNs<Binding>> = BTreeMap::new();
    for (name, spaces) in &module.cfg_gated {
        for (ns, conditions) in spaces {
            let mut slot = explicit
                .get(name)
                .and_then(|bindings| bindings.get(*ns))
                .filter(|binding| !binding.imports().is_empty())
                .cloned();
            if let ([condition], Some(glob)) = (
                conditions.as_slice(),
                globbed.get(name).and_then(|bindings| bindings.get(*ns)),
            ) {
                let absent = Some(format!("not ({condition})"));
                // 같은 칸의 명시 `use`들의 자기 조건 — 조건 없는 것이 있으면(`None`) 글롭은 늘 가려진다.
                let own: Option<Vec<&str>> = conditions_of
                    .get(name)
                    .and_then(|own| own.get(*ns))
                    .map_or(Some(Vec::new()), |own| {
                        own.iter().map(Option::as_deref).collect()
                    });
                let imports: Vec<Import> = glob
                    .imports()
                    .iter()
                    .filter(|imp| imp.target != format!("{path}::{name}"))
                    .filter(|imp| !gated_vertex(tree, &imp.target))
                    // 글롭의 조건이 아이템의 조건을 함축하면 글롭이 보이는 빌드에는 늘 아이템도 있다.
                    .filter(|imp| !implies(&imp.cfg, condition))
                    .map(|imp| Import {
                        cfg: with_import_cfg(&imp.cfg, &absent),
                        ..imp.clone()
                    })
                    // 아이템이 없는 빌드에서도 같은 칸의 명시 `use`가 그 글롭을 가리면 대안이 아니다.
                    .filter(|imp| {
                        own.as_ref()
                            .is_some_and(|own| !own.iter().any(|c| implies(&imp.cfg, c)))
                    })
                    .collect();
                if let Some(glob) = Binding::of_imports(imports) {
                    // 명시 `use`가 첫 원소다.
                    let explicit_side = slot.replace(glob);
                    if let Some(binding) = explicit_side {
                        merge_slot(&mut slot, binding);
                    }
                }
            }
            if let Some(binding) = slot {
                *out.entry(name.clone()).or_default().slot(*ns) = Some(binding);
            }
        }
    }
    out
}

/// 모듈 `importer`의 글롭 `use full::*`(조건 `cfg`, 가시성 `scope`)이 `full`(=`target`)에서 들이는 이름 —
/// `importer`에서 보이는 재노출·해석되지 않는 임포트·선언 아이템을 이름공간마다 따로 들인다. 같은 이름공간이면
/// 선언 아이템이 이긴다. 이름공간을 정하지 못한 임포트([`PerNs::undetermined`])는 그대로 이름공간을 정하지 못한
/// 칸으로 들인다 — 같은 이름의 선언 아이템이 있어도 남는다(그 아이템과 다른 이름공간에 있다). 펼친 이름은 글롭과
/// 원래 것 가운데 좁은 가시성으로 보이고, 두 `#[cfg]` 조건을 함께 싣는다.
fn glob_bindings(
    target: &Module,
    full: &str,
    importer: &str,
    cfg: &Option<String>,
    scope: &Scope,
) -> BTreeMap<String, PerNs<Binding>> {
    let mut out: BTreeMap<String, PerNs<Binding>> = BTreeMap::new();
    for (name, bindings) in &target.imports {
        let mut slots = PerNs::default();
        for (ns, binding) in bindings.iter() {
            *slots.slot(ns) = reexported(binding, importer, cfg, scope);
        }
        slots.undetermined = bindings
            .undetermined
            .as_ref()
            .and_then(|binding| reexported(binding, importer, cfg, scope));
        if !slots.is_empty() {
            out.insert(name.clone(), slots);
        }
    }
    for name in target.items.iter().chain(target.children.keys()) {
        let seen = target.item_scopes.get(name).cloned().unwrap_or_default();
        if !seen.includes(importer) {
            continue;
        }
        let binding = Binding::Resolved(Import {
            target: format!("{full}::{name}"),
            cfg: cfg.clone(),
            scope: scope.narrower(&seen),
        });
        let slots = out.entry(name.clone()).or_default();
        let spaces: Vec<Ns> = Ns::ALL
            .into_iter()
            .filter(|ns| local_in(target, name, *ns))
            .collect();
        if spaces.is_empty() {
            // 아이템 표가 이름공간을 모르는 이름(이름공간 표 없이 `items`만 채운 모듈)은 이름공간을 정하지 못한
            // 칸에 둔다(관대하게 — 같은 이름의 다른 후보가 없으면 어느 자리에서든 찾는다).
            slots.undetermined = Some(binding);
            continue;
        }
        for ns in spaces {
            // 선언 아이템이 같은 이름공간의 재노출을 이긴다 — `#[cfg]`를 단 아이템이 없는 빌드의 대안
            // ([`Module::local_alternatives`])만 함께 들인다(아이템이 첫 원소).
            let alternatives = target
                .local_alternatives
                .get(name)
                .and_then(|bindings| bindings.get(ns))
                .and_then(|alt| reexported(alt, importer, cfg, scope));
            *slots.slot(ns) = alternatives;
            merge_slot(slots.slot(ns), binding.clone());
        }
    }
    out
}

/// `importer`의 글롭이 다른 모듈의 임포트 칸 `binding`을 다시 들인 것 — `importer`에서 보이지 않으면 `None`이다.
/// 가시성은 글롭과 원래 것 가운데 좁은 쪽, 조건은 둘을 함께 싣는다.
fn reexported(
    binding: &Binding,
    importer: &str,
    cfg: &Option<String>,
    scope: &Scope,
) -> Option<Binding> {
    match binding {
        Binding::Unresolved(seen) => seen
            .includes(importer)
            .then(|| Binding::Unresolved(scope.narrower(seen))),
        resolved => Binding::of_imports(
            resolved
                .imports()
                .iter()
                .filter(|imp| imp.scope.includes(importer))
                .map(|imp| Import {
                    target: imp.target.clone(),
                    cfg: conjoin_cfg(cfg, &imp.cfg),
                    scope: scope.narrower(&imp.scope),
                })
                .collect(),
        ),
    }
}

/// 글롭이 펼친 이름 하나를 이름공간마다(이름공간을 정하지 못한 칸 포함) 심는다([`merge_slot`]).
fn merge_glob(globbed: &mut PerNs<Binding>, bindings: PerNs<Binding>) {
    let PerNs {
        types,
        values,
        macros,
        undetermined,
    } = bindings;
    for (slot, binding) in [
        (&mut globbed.types, types),
        (&mut globbed.values, values),
        (&mut globbed.macros, macros),
        (&mut globbed.undetermined, undetermined),
    ] {
        if let Some(binding) = binding {
            merge_slot(slot, binding);
        }
    }
}

/// 한 이름공간의 칸 하나에 소스 순서상 뒤의 임포트 `binding`을 심는다 — 명시 `use`끼리, 글롭끼리 같은 규칙이다
/// (명시 `use`가 글롭을 가리는 것은 [`shadow_globs`]). 해석된 임포트끼리는 대상마다 하나로 합치고(같은 대상이면
/// 가시성은 넓은 쪽, 조건은 둘 중 약한 쪽 — 어느 쪽으로든 들어오면 그 이름이 있다), 대상이 다르면 둘 다 남긴다
/// ([`Binding::Alternatives`] — rustc는 한 빌드에 둘을 두지 않으니 `#[cfg]`로 갈린 대안이다). 첫 원소는 뒤의
/// 것이라 대상 하나를 고르는 해석은 예전처럼 뒤의 것을 고른다. 해석되지 않는 이름끼리는 넓은 가시성으로 합친다.
/// 해석된 것과 해석되지 않는 것이 만나면 소스 순서와 무관하게 해석된 쪽을 남긴다 — 해석되지 않는 쪽(std 경로)은
/// 간선을 낼 대상이 없고, 그것을 남기면 `#[cfg(feature = "pl")] use extdep::Mutex;` 뒤의 `#[cfg(not(..))] use
/// std::sync::Mutex;`가 외부 크레이트 사용 증거를 지워 `deps`가 쓰는 의존을 unused로 냈다.
fn merge_slot(slot: &mut Option<Binding>, binding: Binding) {
    let merged = match (slot.take(), binding) {
        (Some(Binding::Unresolved(seen)), Binding::Unresolved(scope)) => {
            Binding::Unresolved(seen.wider(&scope))
        }
        (Some(seen), binding) => {
            let imports: Vec<Import> = binding
                .imports()
                .iter()
                .chain(seen.imports())
                .cloned()
                .collect();
            // 둘 다 해석되지 않는 경우는 위 팔이 받았으니 합친 목록은 비지 않는다 — `unwrap_or`는 닿지 않는 자리다.
            Binding::of_imports(merge_imports(imports)).unwrap_or(binding)
        }
        (None, binding) => binding,
    };
    *slot = Some(merged);
}

/// 우선 순위 순의 임포트에서 같은 대상을 하나로 합친다 — 자리는 처음 나온 곳, 가시성은 넓은 쪽, 조건은 둘 중
/// 약한 쪽이다.
fn merge_imports(imports: Vec<Import>) -> Vec<Import> {
    let mut out: Vec<Import> = Vec::with_capacity(imports.len());
    for imp in imports {
        match out.iter_mut().find(|seen| seen.target == imp.target) {
            Some(seen) => {
                seen.cfg = weaker_cfg(&seen.cfg, &imp.cfg);
                seen.scope = seen.scope.wider(&imp.scope);
            }
            None => out.push(imp),
        }
    }
    out
}

/// [`fill_imports_settled`]가 돌리는 해석 바퀴의 상한.
pub const MAX_IMPORT_PASSES: usize = 32;

/// 전 모듈의 `use`를 임포트 표가 더 바뀌지 않을 때까지 되풀이해 해석한다 — `modules`는 (모듈 경로,
/// 그 모듈의 아이템). 재노출을 따라가는 해석([`ModTree::resolve`])은 다른 모듈의 임포트 표를 읽으므로,
/// 한 바퀴로는 처리 순서상 뒤에 오는 모듈의 재노출(`pub use zz::Tr`이 거치는 `zz`의 `pub use`)을
/// 못 본다. 바퀴는 [`MAX_IMPORT_PASSES`]까지 — 돌려주는 값은 마지막 바퀴에서도 임포트 표가 바뀐
/// 모듈 수다(0이면 수렴). 호출자는 0이 아니면 limitation으로 센다.
///
/// 앞 바퀴의 해석이 나중에 거둬지면(다른 모듈의 글롭이 아직 펼쳐지지 않아 크레이트 루트로 넘어간 `pub use foo::bar`가
/// 다음 바퀴에 제 대상을 찾으면) 거둔 대상이 서로를 글롭으로 들이는 모듈 사이를 돌며 수렴한 표에 남는다 — 대안
/// 임포트를 남기는 표에서는 거짓 대안이 된다. 그런 일이 있었으면 수렴한 표를 얼려 두고, 모든 모듈의 표를 비워 다시
/// 쌓는다 — 명시 `use`와 글롭 대상은 언 표로 해석하고 글롭이 펼치는 이름만 새로 쌓으니, 근거 없이 서로만 받치던 대상은
/// 다시 생기지 않는다. 그 다음 지금 표로 한 번 더 수렴시켜(대개 한 바퀴) 거두는 것이 없으면 끝이다 — 언 표로 해석한
/// 재노출 사슬의 고리가 남아 거두면 다시 쌓는다. 이것을 [`MAX_IMPORT_PASSES`]번까지 되풀이하고, 그래도 거두는 모듈 수는
/// [`ModTree::stale_import_modules`]에 남긴다.
///
/// 2015 판 크레이트([`ModTree::edition_2015`])의 모듈은 거둔 것을 세지 않고, 다시 쌓을 때도 지금 표로 해석한다 — 그
/// 판의 크레이트 기준 `use` 경로에서는 첫 바퀴에 크레이트 루트로 넘어간 대상이 제 대상일 수 있다. 빈 표에서 지금 표로
/// 다시 쌓으면 그 모듈들은 처음과 같은 이른 대상을 다시 얻고, 이미 깨끗해진 의존 크레이트에서 들인 거둔 대상만 잃는다.
/// 루트가 아닌 모듈의 `use` 경로가 크레이트 루트가 선언한 이름으로 시작하는 2015 판 크레이트는
/// [`ModTree::edition_2015_crate_relative`]에 남긴다.
pub fn fill_imports_settled(
    tree: &mut ModTree,
    modules: &[(String, Vec<&syn::Item>)],
    dep_crates: &DepCrates,
) -> usize {
    fill_imports_settled_within(tree, modules, dep_crates, MAX_IMPORT_PASSES)
}

/// [`fill_imports_settled`]의 본체 — 다시 쌓기를 `max_rebuilds`번까지 한다(테스트가 상한을 낮춰 남은 수를 본다).
fn fill_imports_settled_within(
    tree: &mut ModTree,
    modules: &[(String, Vec<&syn::Item>)],
    dep_crates: &DepCrates,
    max_rebuilds: usize,
) -> usize {
    tree.stale_import_modules = 0;
    tree.edition_2015_crate_relative = crate_relative_2015(tree, modules);
    let (mut changed, mut retracting) = settle_imports(tree, None, modules, dep_crates);
    let mut rounds = 0;
    while changed == 0 && retracting > 0 && rounds < max_rebuilds {
        rounds += 1;
        let frozen = tree.clone();
        for (path, _) in modules {
            if let Some(module) = tree.modules.get_mut(path) {
                module.imports.clear();
                module.glob_targets.clear();
            }
        }
        changed = settle_imports(tree, Some(&frozen), modules, dep_crates).0;
        if changed > 0 {
            break;
        }
        // 깨끗하게 쌓은 표에서 지금 표로 다시 해석해 거두는 것이 없으면 끝이다 — 처음 수렴과 같은 판정이고, 대개
        // 한 바퀴다(언 표로 해석한 재노출 사슬의 고리가 남아 있으면 거두므로 한 번 더 깨끗하게 쌓는다).
        (changed, retracting) = settle_imports(tree, None, modules, dep_crates);
    }
    if changed == 0 && retracting > 0 {
        tree.stale_import_modules = retracting;
    }
    changed
}

/// 루트가 아닌 모듈의 `use` 경로가 크레이트 루트가 선언한 이름(아이템·자식 모듈)으로 시작하는 2015 판 크레이트 —
/// 그 판은 그 경로를 크레이트 루트부터 풀지만 이 해석기는 그 모듈부터 푼다. `crate::`·`self::`·`super::`·`::`로
/// 시작하는 경로는 두 규칙이 같다.
fn crate_relative_2015(tree: &ModTree, modules: &[(String, Vec<&syn::Item>)]) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for (path, items) in modules {
        let krate = crate_of(path);
        if *path == krate || !tree.edition_2015.contains(&krate) || out.contains(&krate) {
            continue;
        }
        let Some(root) = tree.modules.get(&krate) else {
            continue;
        };
        let mut found = false;
        for item in items {
            let syn::Item::Use(u) = item else {
                continue;
            };
            if u.leading_colon.is_some() {
                continue;
            }
            flatten_use(&u.tree, &mut Vec::new(), &mut |segs| {
                let first = segs.first().map(String::as_str).unwrap_or_default();
                found |= !matches!(first, "crate" | "self" | "super" | "*")
                    && (root.items.contains(first) || root.children.contains_key(first));
            });
        }
        if found {
            out.insert(krate);
        }
    }
    out
}

/// [`fill_imports_settled`]의 한 수렴 — 바퀴를 [`MAX_IMPORT_PASSES`]까지 돌려 (마지막 바퀴에서 표가 바뀐 모듈 수,
/// 2015 판이 아니면서 어느 바퀴에서든 앞 바퀴의 해석된 대상을 거둔 모듈 수)를 돌려준다. `frozen`은
/// [`fill_imports_from`]과 같되 2015 판 모듈은 늘 지금 표로 해석한다.
fn settle_imports(
    tree: &mut ModTree,
    frozen: Option<&ModTree>,
    modules: &[(String, Vec<&syn::Item>)],
    dep_crates: &DepCrates,
) -> (usize, usize) {
    let mut changed = 0;
    let mut retracting: BTreeSet<&str> = BTreeSet::new();
    for _ in 0..MAX_IMPORT_PASSES {
        changed = 0;
        for (path, items) in modules {
            let krate = crate_of(path);
            let legacy = tree.edition_2015.contains(&krate);
            let (imports, glob_targets) =
                fill_imports_from(tree, frozen.filter(|_| !legacy), path, items, dep_crates);
            let Some(module) = tree.modules.get(path) else {
                continue;
            };
            if module.imports == imports && module.glob_targets == glob_targets {
                continue;
            }
            changed += 1;
            if !legacy && retracts(&imports, &module.imports) {
                retracting.insert(path);
            }
        }
        if changed == 0 {
            break;
        }
    }
    (changed, retracting.len())
}

/// 모듈 임포트 표가 `before`에서 `after`로 바뀌며 해석된 대상을 거뒀는가 — 앞에 있던 (이름, 대상)이 뒤에 없다.
/// 해석되지 않던 이름이 해석되는 것은 거두는 것이 아니다. 글롭 대상(`glob_targets`)은 보지 않는다 — 거둔 글롭 대상이
/// 들이던 이름이 있었으면 그 이름이 여기서 잡히고, 없었으면 다른 모듈로 퍼질 것이 없다(글롭 대상 자신은 그 모듈의
/// 의존 증거일 뿐 다른 모듈이 펼치지 않는다).
fn retracts(
    before: &BTreeMap<String, PerNs<Binding>>,
    after: &BTreeMap<String, PerNs<Binding>>,
) -> bool {
    let targets = |bindings: &PerNs<Binding>| -> BTreeSet<String> {
        bindings
            .bound()
            .flat_map(Binding::imports)
            .map(|imp| imp.target.clone())
            .collect()
    };
    before.iter().any(|(name, bindings)| {
        let now = after.get(name).map(targets).unwrap_or_default();
        !targets(bindings).is_subset(&now)
    })
}

/// 두 `#[cfg]` 조건이 함께 성립해야 하는 조건 — 글롭 `use`의 조건과 그 글롭이 펼친 재노출의 조건.
/// 조건은 맨 위 `all(..)`을 펼친 조건들의 정렬된 집합으로 합쳐 `all(a , b)`로 쓴다 — 같은 조건을
/// 다시 합쳐도 그대로라, 서로를 글롭으로 들이는 모듈에서도 표가 수렴한다.
fn conjoin_cfg(a: &Option<String>, b: &Option<String>) -> Option<String> {
    let mut all = BTreeSet::new();
    for cfg in [a, b].into_iter().flatten() {
        push_conjuncts(cfg.trim(), &mut all);
    }
    match all.len() {
        0 => None,
        1 => all.into_iter().next(),
        _ => Some(format!(
            "all({})",
            all.into_iter().collect::<Vec<_>>().join(" , ")
        )),
    }
}

/// 같은 대상을 들이는 두 글롭 임포트의 조건 — 어느 쪽으로든 들어오면 임포트가 있으므로 약한 쪽이다.
/// 한쪽 조건들이 다른 쪽의 부분 집합이면 그쪽, 아니면(비교할 수 없으면) 조건 없음으로 둔다 — 간선을
/// 남기는 쪽이라 거짓 unreachable을 만들지 않는다.
fn weaker_cfg(a: &Option<String>, b: &Option<String>) -> Option<String> {
    let (Some(a), Some(b)) = (a, b) else {
        return None;
    };
    let (mut ca, mut cb) = (BTreeSet::new(), BTreeSet::new());
    push_conjuncts(a.trim(), &mut ca);
    push_conjuncts(b.trim(), &mut cb);
    if ca.is_subset(&cb) {
        Some(a.clone())
    } else if cb.is_subset(&ca) {
        Some(b.clone())
    } else {
        None
    }
}

/// 조건을 맨 위 `all(..)`(또는 `all (..)`)의 조건들로 펼쳐 넣는다 — 안쪽 `all`도 펼친다. `any`·`not`과
/// 술어는 그대로 한 조건이다.
fn push_conjuncts(cfg: &str, out: &mut BTreeSet<String>) {
    match all_arguments(cfg) {
        Some(inner) => {
            for part in split_top_level(inner) {
                push_conjuncts(part.trim(), out);
            }
        }
        None if !cfg.is_empty() => {
            out.insert(cfg.to_string());
        }
        None => {}
    }
}

/// `all(..)`·`all (..)`이면 괄호 안 — 여는 괄호의 짝이 마지막 글자일 때만이다.
fn all_arguments(cfg: &str) -> Option<&str> {
    call_arguments(cfg, "all")
}

/// `name(..)`·`name (..)`이면 괄호 안 — 여는 괄호의 짝이 마지막 글자일 때만이다.
fn call_arguments<'a>(cfg: &'a str, name: &str) -> Option<&'a str> {
    let inner = cfg.strip_prefix(name)?.trim_start().strip_prefix('(')?;
    let mut depth = 1usize;
    let mut quoted = false;
    let mut escaped = false;
    for (i, c) in inner.char_indices() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return (i + 1 == inner.len()).then(|| &inner[..i]);
                }
            }
            _ => {}
        }
    }
    None
}

/// 괄호·문자열 밖의 쉼표로 나눈다.
fn split_top_level(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0usize, 0usize);
    let (mut quoted, mut escaped) = (false, false);
    for (i, c) in s.char_indices() {
        if quoted {
            match (escaped, c) {
                (true, _) => escaped = false,
                (false, '\\') => escaped = true,
                (false, '"') => quoted = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => quoted = true,
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

/// use 경로 끝 세그먼트를 붙인다. 그룹 안의 `self`(`use a::b::{self}`·`{self as x}`)는 그룹 앞
/// 경로 자체라 붙이지 않는다 — 임포트 이름은 그 마지막 세그먼트(`b`)나 별칭이다.
fn push_use_segment(path: &mut Vec<String>, ident: &syn::Ident) {
    if ident != "self" || path.is_empty() {
        path.push(ident.to_string());
    }
}

/// `use` 트리를 평탄한 경로 목록으로 펼친다 — `{a, b}` 그룹과 `as` 별칭을 처리한다.
/// 경로마다 use 아이템의 cfg를 붙여야 해서 결과는 콜백으로 흘려보낸다.
fn flatten_use(tree: &syn::UseTree, prefix: &mut Vec<String>, out: &mut dyn FnMut(Vec<String>)) {
    match tree {
        syn::UseTree::Path(p) => {
            prefix.push(p.ident.to_string());
            flatten_use(&p.tree, prefix, out);
            prefix.pop();
        }
        syn::UseTree::Name(n) => {
            let mut p = prefix.clone();
            push_use_segment(&mut p, &n.ident);
            out(p);
        }
        syn::UseTree::Rename(r) => {
            let mut p = prefix.clone();
            push_use_segment(&mut p, &r.ident);
            p.push(format!("as {}", r.rename));
            out(p);
        }
        syn::UseTree::Glob(_) => {
            // glob은 모듈 자체 + 그 아이템 전부 — `*` 마커를 남겨
            // fill_imports가 아이템별로 펼치게 한다.
            let mut p = prefix.clone();
            p.push("*".to_string());
            out(p);
        }
        syn::UseTree::Group(g) => {
            for t in &g.items {
                flatten_use(t, prefix, out);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 이름공간 `spaces`에 `target`으로 묶인 임포트 — 조건 없고 공개다.
    fn bound(target: &str, spaces: &[Ns]) -> PerNs<Binding> {
        let mut bindings = PerNs::default();
        for ns in spaces {
            *bindings.slot(*ns) = Some(Binding::Resolved(Import {
                target: target.to_string(),
                cfg: None,
                scope: Scope::Public,
            }));
        }
        bindings
    }

    /// 모듈 `module`이 이름공간 `ns`에서 `name`으로 들인 해석된 임포트.
    fn import_in<'a>(t: &'a ModTree, module: &str, name: &str, ns: Ns) -> Option<&'a Import> {
        t.modules[module]
            .imports
            .get(name)?
            .get(ns)?
            .imports()
            .first()
    }

    /// 최소 트리: c 루트(items: f, S), c::m(items: g, 임포트 h→c::util::h).
    fn tree() -> ModTree {
        let mut t = ModTree::default();
        let mut root = Module::new(PathBuf::from("lib.rs"), true, true);
        root.items = BTreeSet::from(["f".to_string(), "S".to_string(), "m".to_string()]);
        root.children.insert("m".to_string(), "c::m".to_string());
        t.modules.insert("c".to_string(), root);
        let mut m = Module::new(PathBuf::from("m.rs"), true, true);
        m.items = BTreeSet::from(["g".to_string()]);
        m.imports
            .insert("h".to_string(), bound("c::util::h", &Ns::ALL));
        t.modules.insert("c::m".to_string(), m);
        let mut u = Module::new(PathBuf::from("util.rs"), true, true);
        u.items = BTreeSet::from(["h".to_string()]);
        t.modules.insert("c::util".to_string(), u);
        t
    }

    #[test]
    fn resolve_crate_self_super_and_items() {
        let t = tree();
        let none = DepCrates::new();
        // crate:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["crate".into(), "f".into()], &none),
            Some("c::f".to_string())
        );
        // self:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["self".into(), "g".into()], &none),
            Some("c::m::g".to_string())
        );
        // super:: 접두사.
        assert_eq!(
            t.resolve("c::m", &["super".into(), "f".into()], &none),
            Some("c::f".to_string())
        );
        // 모듈 로컬 아이템.
        assert_eq!(
            t.resolve("c", &["f".into()], &none),
            Some("c::f".to_string())
        );
    }

    /// 가시성을 모듈 범위로 — 제한 가시성은 가리키는 모듈을 찾지 못해도(루트를 넘는 `super`) 크레이트
    /// 전체에 그친다. 컴파일되는 코드는 이 경로에 오지 않아 통합 테스트로는 닿지 않는다.
    #[test]
    fn scope_of_restricted_visibility_never_widens_past_the_crate() {
        let scope = |vis: &str, module: &str| {
            let vis: syn::Visibility = syn::parse_str(vis).expect("valid visibility");
            scope_of(&vis, module)
        };
        let within = |m: &str| Scope::Within(m.to_string());
        assert_eq!(scope("pub", "c::m"), Scope::Public);
        assert_eq!(scope("", "c::m"), within("c::m"));
        assert_eq!(scope("pub(self)", "c::m"), within("c::m"));
        assert_eq!(scope("pub(crate)", "c::m::n"), within("c"));
        assert_eq!(scope("pub(super)", "c::m::n"), within("c::m"));
        assert_eq!(scope("pub(in crate::m)", "c::m::n"), within("c::m"));
        // 2015 판의 크레이트 기준 경로.
        assert_eq!(scope("pub(in m)", "c::m::n"), within("c::m"));
        // 루트를 넘는 `super`는 크레이트 전체로 — 어디서나 보이는 것으로 떨어지지 않는다.
        assert_eq!(scope("pub(super)", "c"), within("c"));
        assert_eq!(scope("pub(in super::super)", "c::m"), within("c"));
    }

    /// 서로를 재노출 글롭으로 들이는 두 모듈(`a`의 `#[cfg(fa)] pub use crate::b::*`, `b`의
    /// `#[cfg(fb)] pub use crate::a::*`)도 임포트 표가 수렴한다 — 펼친 재노출의 조건은 조건들의 집합으로
    /// 합쳐 같은 조건을 다시 합쳐도 그대로다(문자열로 겹쳐 붙이면 `all(fb , all(fa , …))`로 끝없이
    /// 자란다). 대상은 정의 위치(`c::src::x`)이고, `a`는 `src`의 글롭으로 조건 없이 `x`를 받으므로 조건이
    /// 없다(같은 대상을 들이는 글롭 가운데 약한 조건).
    #[test]
    fn mutual_globs_with_cfg_settle() {
        let mut t = ModTree::default();
        let mut root = Module::new(PathBuf::from("lib.rs"), true, true);
        for child in ["a", "b", "src"] {
            root.items.insert(child.to_string());
            root.children
                .insert(child.to_string(), format!("c::{child}"));
        }
        t.modules.insert("c".to_string(), root);
        for child in ["a", "b", "src"] {
            t.modules.insert(
                format!("c::{child}"),
                Module::new(PathBuf::from(format!("{child}.rs")), true, true),
            );
        }
        let src = t.modules.get_mut("c::src").expect("fixture module");
        src.items.insert("x".to_string());
        src.value_items.insert("x".to_string());
        let a: syn::File =
            syn::parse_str("pub use crate::src::*; #[cfg(feature = \"fa\")] pub use crate::b::*;")
                .expect("valid use items");
        let b: syn::File = syn::parse_str("#[cfg(feature = \"fb\")] pub use crate::a::*;")
            .expect("valid use items");
        let modules = vec![
            ("c::a".to_string(), a.items.iter().collect::<Vec<_>>()),
            ("c::b".to_string(), b.items.iter().collect::<Vec<_>>()),
        ];
        assert_eq!(fill_imports_settled(&mut t, &modules, &DepCrates::new()), 0);
        for module in ["c::a", "c::b"] {
            let x = import_in(&t, module, "x", Ns::Value).expect("x in the value namespace");
            assert_eq!(x.target, "c::src::x");
            // 값 아이템 `x`는 타입·매크로 이름공간에 들어오지 않는다.
            assert!(t.modules[module].imports["x"].types.is_none());
            assert!(t.modules[module].imports["x"].macros.is_none());
        }
        let cfg = |module| import_in(&t, module, "x", Ns::Value).and_then(|x| x.cfg.as_deref());
        assert_eq!(cfg("c::a"), None);
        assert_eq!(cfg("c::b"), Some("feature = \"fb\""));
    }

    /// 그룹 안의 `self`(`use crate::util::{self, h as hh}`·`{self as u2}`)는 그룹 앞 경로 자체를
    /// 들인다 — 이름은 그 마지막 세그먼트나 별칭이다. `self`를 세그먼트로 붙여 해석하면
    /// `c::util::self`가 없어 임포트가 조용히 사라지고, `util::h()`와 모듈 `uses` 간선이 빠진다.
    /// 임포트 표는 그 모듈의 `use`로 새로 만든다 — fixture가 미리 둔 `h` 임포트는 남지 않는다.
    #[test]
    fn use_group_self_imports_the_prefix_module() {
        let mut t = tree();
        let none = DepCrates::new();
        let file: syn::File =
            syn::parse_str("use crate::util::{self, h as hh}; use crate::util::{self as u2};")
                .expect("valid use items");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_imports(&mut t, "c::m", &items, &none);
        let imports = &t.modules["c::m"].imports;
        // 이 fixture의 아이템 표는 이름공간을 모른다(이름공간을 정하지 못한 칸에 있다) — 대상마다 한 번씩 센다.
        let targets: BTreeSet<(&str, &str)> = imports
            .iter()
            .flat_map(|(alias, bindings)| {
                bindings
                    .bound()
                    .flat_map(Binding::imports)
                    .map(move |imp| (alias.as_str(), imp.target.as_str()))
            })
            .collect();
        assert_eq!(
            targets,
            BTreeSet::from([("hh", "c::util::h"), ("u2", "c::util"), ("util", "c::util")])
        );
        assert_eq!(
            t.resolve("c::m", &["util".into(), "h".into()], &none),
            Some("c::util::h".to_string())
        );
    }

    /// 뒤에 세그먼트가 이어지는 첫 세그먼트는 타입 이름공간이다 — 같은 모듈의 값 아이템
    /// (`fn util`)이 같은 이름으로 들인 모듈(`use crate::util`)을 가리지 않는다. 한 세그먼트
    /// 값 자리(`util(1)`)는 지금처럼 그 fn이다. 로컬에 같은 이름의 타입 이름공간 아이템이 없으니
    /// 접두사 자리는 그 임포트다.
    #[test]
    fn value_item_does_not_shadow_an_imported_module_prefix() {
        let mut t = tree();
        let none = DepCrates::new();
        let m = t.modules.get_mut("c::m").expect("fixture module");
        m.items.insert("util".to_string());
        m.value_items.insert("util".to_string());
        m.imports
            .insert("util".to_string(), bound("c::util", &[Ns::Type]));
        assert_eq!(
            t.resolve("c::m", &["util".into(), "h".into()], &none),
            Some("c::util::h".to_string())
        );
        assert_eq!(
            t.resolve("c::m", &["util".into()], &none),
            Some("c::m::util".to_string())
        );
        // 자리를 알면 그 이름공간에 묶인 것이다 — 값은 로컬 fn, 타입은 들인 모듈.
        assert_eq!(
            t.resolve_in("c::m", &["util".into()], &none, Ns::Value),
            Some("c::m::util".to_string())
        );
        assert_eq!(
            t.resolve_in("c::m", &["util".into()], &none, Ns::Type),
            Some("c::util".to_string())
        );
    }

    /// 로컬 타입 이름공간 아이템은 같은 이름의 임포트(글롭)를 가린다 — 같은 이름의 fn이 함께 있어도
    /// (`pub struct util {}`와 `pub fn util()`은 이름공간이 달라 공존한다) 접두사 `util::h`는 로컬
    /// 구조체의 연관 경로이지 글롭이 들인 `c::util` 모듈이 아니다. 연관 아이템은 아이템 표에 없어
    /// 해석은 None이고, 엉뚱한 `c::util::h`로 확정되지 않는다.
    #[test]
    fn local_type_item_shadows_a_glob_import_beside_a_same_named_fn() {
        let mut t = tree();
        let none = DepCrates::new();
        let file: syn::File =
            syn::parse_str("pub struct util {} pub fn util() {}").expect("valid items");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_items(&mut t, "c::m", &items);
        t.modules
            .get_mut("c::m")
            .expect("fixture module")
            .imports
            .insert("util".to_string(), bound("c::util", &[Ns::Type]));
        assert_eq!(t.resolve("c::m", &["util".into(), "h".into()], &none), None);
        assert_eq!(
            t.resolve("c::m", &["util".into()], &none),
            Some("c::m::util".to_string())
        );
    }

    /// 이름공간을 정하지 못한 임포트(`pub use ext_derive::Foo`가 해석되지 않은 것)는 같은 이름의 로컬 아이템이나
    /// 이름공간이 정해진 임포트가 있으면 타입·값 자리에서 이기지 못한다 — rustc에서 그 `use`는 그것들과 다른
    /// 이름공간에 있다(E0255·E0252). 같은 근거로 매크로 자리에서는 그 `use`가 유일한 후보다 — 로컬 trait `Foo`와
    /// 글롭이 들인 trait `Bar`는 매크로가 될 수 없다(`#[derive(Foo)]`는 derive 매크로다). 다른 후보가 없으면 그
    /// `use`가 이름의 해석을 멈춘다(크레이트 루트로 넘어가지 않는다). 자리를 모르는 해석은 정해진 임포트를 먼저
    /// 고른다.
    #[test]
    fn undetermined_import_yields_to_local_items_and_determined_imports() {
        let mut t = tree();
        let none = DepCrates::new();
        let undetermined = || PerNs::undetermined_only(Binding::Unresolved(Scope::Public));
        let m = t.modules.get_mut("c::m").expect("fixture module");
        // 로컬 trait `Foo`(타입) 옆의 이름공간 모르는 `Foo`.
        m.items.insert("Foo".to_string());
        m.type_items.insert("Foo".to_string());
        m.imports.insert("Foo".to_string(), undetermined());
        // 글롭이 들인 trait `Bar`(타입, 정해짐) 옆의 이름공간 모르는 `Bar` — 해석된 쪽은 외부 크레이트로 붕괴한 것.
        let mut bar = bound("c::util::h", &[Ns::Type]);
        bar.undetermined = Some(Binding::Resolved(Import {
            target: "ext".to_string(),
            cfg: None,
            scope: Scope::Public,
        }));
        m.imports.insert("Bar".to_string(), bar);
        // 다른 후보가 없는 `f` — 크레이트 루트에 `f`가 있어도 넘어가지 않는다.
        m.imports.insert("f".to_string(), undetermined());
        for ns in [Ns::Type, Ns::Value] {
            assert_eq!(
                t.resolve_in("c::m", &["Foo".into()], &none, ns),
                Some("c::m::Foo".to_string()),
                "{ns:?}"
            );
            assert_eq!(
                t.resolve_in("c::m", &["Bar".into()], &none, ns),
                Some("c::util::h".to_string()),
                "{ns:?}"
            );
        }
        // 매크로 자리: 해석되지 않는 derive 재노출이 해석을 멈추고, 해석된 쪽은 그 외부 크레이트 정점이다(`--deps`).
        assert_eq!(
            t.resolve_in("c::m", &["Foo".into()], &none, Ns::Macro),
            None
        );
        let mut deps = DepCrates::new();
        deps.external.insert("ext".to_string());
        assert_eq!(
            t.resolve_in("c::m", &["Bar".into()], &deps, Ns::Macro),
            Some("ext".to_string())
        );
        for ns in Ns::ALL {
            assert_eq!(t.resolve_in("c::m", &["f".into()], &none, ns), None);
        }
        assert_eq!(
            t.resolve("c::m", &["Bar".into()], &none),
            Some("c::util::h".to_string())
        );
        assert_eq!(t.resolve("c::m", &["f".into()], &none), None);
    }

    /// 같은 이름을 들이는 명시 `use` 둘 가운데 하나만 이름공간이 정해지면(`pub use crate::split_at::SplitAt`과
    /// derive 재노출 `pub use ext_derive::SplitAt`) 정해진 칸은 소스 순서와 무관하게 남는다. 이름공간을 모르는
    /// 명시 `use`는 같은 이름의 글롭을 모두 가린다.
    #[test]
    fn determined_explicit_import_survives_an_undetermined_one_in_either_order() {
        for order in [
            "use crate::util::h as x; use std::x;",
            "use std::x; use crate::util::h as x;",
        ] {
            let mut t = tree();
            let none = DepCrates::new();
            t.modules
                .get_mut("c::util")
                .expect("fixture module")
                .value_items
                .insert("h".to_string());
            let file: syn::File = syn::parse_str(&format!("use crate::*; use std::f; {order}"))
                .expect("valid use items");
            let items: Vec<&syn::Item> = file.items.iter().collect();
            fill_imports(&mut t, "c::m", &items, &none);
            let imports = &t.modules["c::m"].imports;
            assert_eq!(
                import_in(&t, "c::m", "x", Ns::Value).map(|i| i.target.as_str()),
                Some("c::util::h"),
                "{order}"
            );
            assert!(matches!(
                imports["x"].undetermined,
                Some(Binding::Unresolved(_))
            ));
            // 글롭 `use crate::*`가 들인 `f`는 이름공간 모르는 `use std::f`에 통째로 가려지고, `S`는 남는다.
            assert!(!imports["f"].has_determined());
            assert!(matches!(
                imports["f"].undetermined,
                Some(Binding::Unresolved(_))
            ));
            assert!(matches!(
                imports["S"].undetermined,
                Some(Binding::Resolved(_))
            ));
        }
    }

    /// `tree()`에 `c::alt`(`#[cfg(unix)] pub use crate::util::h as x;`와 `#[cfg(windows)] pub use crate::m::g as x;`)를
    /// 더하고 그 `use`로 임포트 표를 채운다 — `h`·`g`는 값 아이템이다.
    fn tree_with_split_reexport() -> ModTree {
        let mut t = tree();
        for (module, name) in [("c::util", "h"), ("c::m", "g")] {
            t.modules
                .get_mut(module)
                .expect("fixture module")
                .value_items
                .insert(name.to_string());
        }
        t.modules
            .get_mut("c")
            .expect("fixture root")
            .children
            .insert("alt".to_string(), "c::alt".to_string());
        t.modules.insert(
            "c::alt".to_string(),
            Module::new(PathBuf::from("alt.rs"), true, true),
        );
        let file: syn::File = syn::parse_str(
            "#[cfg(unix)] pub use crate::util::h as x; #[cfg(windows)] pub use crate::m::g as x;",
        )
        .expect("valid use items");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_imports(&mut t, "c::alt", &items, &DepCrates::new());
        t
    }

    /// 같은 이름공간에 같은 이름을 들이는 명시 `use` 둘이 다른 대상을 가리키면 둘 다 남는다 — 첫 원소는 소스
    /// 순서상 뒤의 것이라 대상 하나를 고르는 해석(`resolve`)은 예전처럼 그것이고, `resolve_all`은 둘 다를 각자의
    /// 조건과 함께 준다. 예전에는 뒤의 것만 남아 `crate::util::h`로 가는 간선이 사라졌다.
    #[test]
    fn cfg_split_explicit_imports_are_alternatives() {
        let t = tree_with_split_reexport();
        let none = DepCrates::new();
        let slot = t.modules["c::alt"].imports["x"]
            .get(Ns::Value)
            .expect("x in the value namespace");
        let targets: Vec<(&str, Option<&str>)> = slot
            .imports()
            .iter()
            .map(|imp| (imp.target.as_str(), imp.cfg.as_deref()))
            .collect();
        assert_eq!(
            targets,
            [("c::m::g", Some("windows")), ("c::util::h", Some("unix"))]
        );
        let path = ["alt".to_string(), "x".to_string()];
        assert_eq!(t.resolve("c", &path, &none), Some("c::m::g".to_string()));
        let all = t.resolve_all("c", &path, &none, Some(Ns::Value));
        assert_eq!(
            all,
            [
                Resolved {
                    target: "c::m::g".to_string(),
                    cfg: Some("windows".to_string()),
                },
                Resolved {
                    target: "c::util::h".to_string(),
                    cfg: Some("unix".to_string()),
                },
            ]
        );
    }

    /// 다른 모듈이 그 이름을 `use`하면 대안이 그대로 넘어가고, 각 임포트의 조건은 그 `use`의 조건과 거쳐 온
    /// 재노출의 조건을 함께 싣는다.
    #[test]
    fn importing_a_split_reexport_keeps_each_alternative_with_its_cfg() {
        let mut t = tree_with_split_reexport();
        let file: syn::File =
            syn::parse_str("#[cfg(feature = \"f\")] use crate::alt::x;").expect("valid use item");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_imports(&mut t, "c::m", &items, &DepCrates::new());
        let slot = t.modules["c::m"].imports["x"]
            .get(Ns::Value)
            .expect("x in the value namespace");
        let targets: Vec<(&str, Option<&str>)> = slot
            .imports()
            .iter()
            .map(|imp| (imp.target.as_str(), imp.cfg.as_deref()))
            .collect();
        assert_eq!(
            targets,
            [
                ("c::m::g", Some("all(feature = \"f\" , windows)")),
                ("c::util::h", Some("all(feature = \"f\" , unix)")),
            ]
        );
    }

    /// 대상 하나를 고르는 해석은 우선 순위가 가장 높은 대안에서 경로가 끊기면 다음 대안으로 넘어간다 — `x::y`의
    /// `y`가 뒤의 대안(`c::m`)에는 없고 앞의 대안(`c::util`)에만 있다.
    #[test]
    fn resolve_falls_back_to_the_next_alternative_when_the_first_path_ends() {
        let mut t = tree();
        let none = DepCrates::new();
        t.modules
            .get_mut("c::util")
            .expect("fixture module")
            .items
            .insert("y".to_string());
        // `util`도 루트의 자식 모듈이어야 두 별칭이 같은 타입 이름공간의 대안이 된다.
        t.modules
            .get_mut("c")
            .expect("fixture root")
            .children
            .insert("util".to_string(), "c::util".to_string());
        let file: syn::File =
            syn::parse_str("#[cfg(unix)] use crate::util as x; #[cfg(windows)] use crate::m as x;")
                .expect("valid use items");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_imports(&mut t, "c", &items, &none);
        let path = ["x".to_string(), "y".to_string()];
        assert_eq!(t.resolve("c", &path, &none), Some("c::util::y".to_string()));
        assert_eq!(
            t.resolve_all("c", &path, &none, None),
            [Resolved {
                target: "c::util::y".to_string(),
                cfg: Some("unix".to_string()),
            }]
        );
    }

    /// 같은 대상을 들이는 명시 `use` 둘은 하나로 합친다 — 조건은 약한 쪽(비교할 수 없으면 조건 없음), 가시성은
    /// 넓은 쪽이다. 예전에는 뒤의 `use`의 조건으로 덮어 `--target`이 앞의 빌드에서도 그 `uses` 간선을 지웠다.
    #[test]
    fn same_target_explicit_imports_merge_to_the_weaker_cfg() {
        let mut t = tree();
        let file: syn::File = syn::parse_str(
            "#[cfg(unix)] use crate::util::h; #[cfg(windows)] pub(crate) use crate::util::h;",
        )
        .expect("valid use items");
        let items: Vec<&syn::Item> = file.items.iter().collect();
        fill_imports(&mut t, "c::m", &items, &DepCrates::new());
        let binding = t.modules["c::m"].imports["h"]
            .undetermined
            .as_ref()
            .expect("h has no namespace table in this fixture");
        assert_eq!(
            binding,
            &Binding::Resolved(Import {
                target: "c::util::h".to_string(),
                cfg: None,
                scope: Scope::Within("c".to_string()),
            })
        );
    }

    /// `(모듈 경로, 소스)`로 트리를 만들고 아이템 표를 채운다 — 자식 관계는 경로로 잇는다. 돌려주는 파일은 임포트
    /// 해석의 입력이다.
    fn tree_from(modules: &[(&str, &str)]) -> (ModTree, Vec<(String, syn::File)>) {
        let mut t = ModTree::default();
        let mut files = Vec::new();
        for (path, src) in modules {
            t.modules.insert(
                path.to_string(),
                Module::new(PathBuf::from(format!("{path}.rs")), true, true),
            );
            if let Some((parent, name)) = path.rsplit_once("::") {
                t.modules
                    .get_mut(parent)
                    .expect("parents come first")
                    .children
                    .insert(name.to_string(), path.to_string());
            }
            files.push((
                path.to_string(),
                syn::parse_str::<syn::File>(src).expect("valid items"),
            ));
        }
        for (path, file) in &files {
            let items: Vec<&syn::Item> = file.items.iter().collect();
            fill_items(&mut t, path, &items);
        }
        (t, files)
    }

    /// `#[cfg]` 로컬 아이템 옆 글롭 이름이 어느 빌드에서도 보이지 않으면 표에 대안으로 두지 않는다 — 글롭의 조건이
    /// 아이템의 조건을 함축하거나(같은 `#[cfg(feature = "x")]`), 글롭 대상이 스스로 조건부다(`#[cfg(test)] mod tests`).
    /// 조건 없는 글롭과 조건 없는 대상이면 부정 조건을 달고 대안이다.
    #[test]
    fn glob_alternatives_skip_globs_that_are_never_visible() {
        let alternatives = |m_src: &str| {
            let (mut t, files) = tree_from(&[
                ("c", "mod plat {} #[cfg(test)] mod tests {} mod m {}"),
                ("c::plat", "pub fn sys() {}"),
                ("c::tests", "pub fn helper() {}"),
                ("c::m", m_src),
            ]);
            let uses: Vec<(String, Vec<&syn::Item>)> = files
                .iter()
                .map(|(path, file)| (path.clone(), file.items.iter().collect()))
                .collect();
            fill_imports_settled(&mut t, &uses, &DepCrates::new());
            let m = &t.modules["c::m"];
            m.local_alternatives
                .values()
                .flat_map(|b| {
                    b.bound()
                        .flat_map(Binding::imports)
                        .cloned()
                        .collect::<Vec<_>>()
                })
                .map(|imp| (imp.target, imp.cfg))
                .collect::<Vec<_>>()
        };
        let same_gate =
            "#[cfg(feature = \"x\")] use crate::plat::*; #[cfg(feature = \"x\")] fn sys() {}";
        assert_eq!(alternatives(same_gate), []);
        let gated_target = "use crate::*; #[cfg(any(test, kani))] mod tests {}";
        assert_eq!(alternatives(gated_target), []);
        let open = "use crate::plat::*; #[cfg(unix)] fn sys() {}";
        assert_eq!(
            alternatives(open),
            [("c::plat::sys".to_string(), Some("not (unix)".to_string()))]
        );
    }

    /// 다시 쌓기 상한 안에서 거둔 대상이 다 지워지지 않으면 그렇게 남은 모듈 수를 [`ModTree::stale_import_modules`]에
    /// 남긴다 — 재노출 사슬(`w1`·`w2`·`w3`)은 한 번에 한 고리씩 깨끗해지므로 상한 한 번으로는 남고, 기본 상한으로는
    /// 0이다.
    #[test]
    fn stale_import_modules_counts_what_the_rebuild_limit_left() {
        let modules: &[(&str, &str)] = &[
            (
                "c",
                "mod x {} mod y {} mod foo {} mod real {} mod z {} mod w1 {} mod w2 {} mod w3 {}",
            ),
            ("c::x", "pub use crate::z::*; pub use crate::y::*;"),
            ("c::y", "pub use crate::x::*;"),
            ("c::foo", "pub fn bar() {}"),
            ("c::real", "pub mod foo {}"),
            ("c::real::foo", "pub fn bar() {}"),
            ("c::z", "use crate::real::*; pub use foo::bar;"),
            ("c::w1", "pub use crate::x::bar;"),
            ("c::w2", "pub use crate::w1::bar;"),
            ("c::w3", "pub use crate::w2::bar;"),
        ];
        let settle = |limit: usize| {
            let (mut t, files) = tree_from(modules);
            let uses: Vec<(String, Vec<&syn::Item>)> = files
                .iter()
                .map(|(path, file)| (path.clone(), file.items.iter().collect()))
                .collect();
            assert_eq!(
                fill_imports_settled_within(&mut t, &uses, &DepCrates::new(), limit),
                0
            );
            let w3 = t.modules["c::w3"].imports["bar"].get(Ns::Value).map(|b| {
                b.imports()
                    .iter()
                    .map(|i| i.target.clone())
                    .collect::<Vec<_>>()
            });
            (t.stale_import_modules, w3)
        };
        let (stale, _) = settle(1);
        assert!(stale > 0, "one rebuild should leave a stale chain link");
        assert_eq!(
            settle(MAX_IMPORT_PASSES),
            (0, Some(vec!["c::real::foo::bar".to_string()]))
        );
    }

    /// 2015 판 크레이트의 루트가 아닌 모듈에서 크레이트 루트가 선언한 이름으로 시작하는 `use` 경로를 센다 — `crate::`·
    /// `self::`·`super::`·`::`로 시작하거나 루트에 없는 이름이면 두 판의 규칙이 같아 세지 않는다.
    #[test]
    fn crate_relative_use_paths_are_counted_for_edition_2015_only() {
        let count = |use_line: &str, edition_2015: bool| {
            let (mut t, files) = tree_from(&[
                ("c", "mod foo {} mod z {}"),
                ("c::foo", "pub fn bar() {}"),
                ("c::z", use_line),
            ]);
            if edition_2015 {
                t.edition_2015.insert("c".to_string());
            }
            let uses: Vec<(String, Vec<&syn::Item>)> = files
                .iter()
                .map(|(path, file)| (path.clone(), file.items.iter().collect()))
                .collect();
            fill_imports_settled(&mut t, &uses, &DepCrates::new());
            t.edition_2015_crate_relative.len()
        };
        assert_eq!(count("pub use foo::bar;", true), 1);
        assert_eq!(count("pub use foo::bar;", false), 0);
        for same in [
            "pub use crate::foo::bar;",
            "pub use super::foo::bar;",
            "pub use ::std::fmt;",
            "pub use std::fmt;",
        ] {
            assert_eq!(count(same, true), 0, "{same}");
        }
    }

    /// 같은 모듈을 두 번 채우면(아이템 묶음이 둘) 선언을 합친다 — 앞이든 뒤든 조건 없는 선언이 있는 이름공간은
    /// 조건부가 아니고, 둘 다 `#[cfg]`를 달았으면 두 조건을 다 적는다. 자식 모듈 표는 앞선 선언으로 보지 않는다.
    #[test]
    fn filling_a_module_twice_merges_gated_declarations() {
        let gated = |first: &str, second: &str| {
            let mut t = tree();
            t.modules
                .get_mut("c::m")
                .expect("fixture module")
                .children
                .insert("k".to_string(), "c::m::k".to_string());
            for src in [first, second] {
                let file: syn::File = syn::parse_str(src).expect("valid items");
                let items: Vec<&syn::Item> = file.items.iter().collect();
                fill_items(&mut t, "c::m", &items);
            }
            let m = &t.modules["c::m"];
            (
                m.cfg_gated
                    .get("sum")
                    .and_then(|s| s.get(&Ns::Value))
                    .cloned(),
                m.cfg_gated.get("k").and_then(|s| s.get(&Ns::Type)).cloned(),
            )
        };
        let a = "#[cfg(unix)] fn sum() {} #[cfg(feature = \"k\")] mod k {}";
        assert_eq!(gated(a, "fn sum() {}").0, None);
        assert_eq!(gated("fn sum() {}", a).0, None);
        assert_eq!(
            gated(a, "#[cfg(windows)] fn sum() {}").0,
            Some(vec!["unix".to_string(), "windows".to_string()])
        );
        assert_eq!(gated(a, "").1, Some(vec!["feature = \"k\"".to_string()]));
    }

    /// 임포트의 조건은 간선의 조건에 더해진다 — 이미 같은 조건이 있으면 표기를 바꾸지 않는다.
    #[test]
    fn import_cfg_joins_the_edge_cfg() {
        let some = |s: &str| Some(s.to_string());
        assert_eq!(with_import_cfg(&None, &None), None);
        assert_eq!(with_import_cfg(&some("unix"), &None), some("unix"));
        assert_eq!(with_import_cfg(&None, &some("unix")), some("unix"));
        assert_eq!(
            with_import_cfg(&some("all (unix , test)"), &some("test")),
            some("all (unix , test)")
        );
        assert_eq!(
            with_import_cfg(&some("windows"), &some("feature = \"x\"")),
            some("all(feature = \"x\" , windows)")
        );
    }

    #[test]
    fn resolve_uses_imports_and_nested_paths() {
        let t = tree();
        let none = DepCrates::new();
        // use 별칭 → 임포트 대상.
        assert_eq!(
            t.resolve("c::m", &["h".into()], &none),
            Some("c::util::h".to_string())
        );
        // 자식 모듈 경유.
        assert_eq!(
            t.resolve("c", &["m".into(), "g".into()], &none),
            Some("c::m::g".to_string())
        );
        // 워크스페이스의 다른 크레이트 루트.
        assert_eq!(
            t.resolve("c", &["other".into(), "x".into()], &{
                let mut t2 = tree();
                t2.modules.insert(
                    "other".to_string(),
                    Module::new(PathBuf::from("o.rs"), true, true),
                );
                drop(t2);
                DepCrates::new()
            }),
            None // other 크레이트는 modules에 없으니 None.
        );
    }

    #[test]
    fn declared_dep_alias_beats_same_named_workspace_root() {
        // 워크스페이스에 `foo`라는 멤버 루트가 있어도, `c`가 `foo` 별칭을
        // 다른 패키지에 선언했으면 `foo::x`는 선언 쪽으로 간다 —
        // 루트 이름을 먼저 보면 선언된 의존이 엉뚱한 크레이트로 간다.
        let mut t = tree();
        t.modules.insert(
            "foo".to_string(),
            Module::new(PathBuf::from("foo/lib.rs"), true, true),
        );
        let map = dep_scope("foo", "real_pkg", false);
        assert_eq!(
            t.resolve("c", &["foo".into(), "x".into()], &map),
            Some("real_pkg".to_string())
        );
        // 선언이 없는 크레이트에서의 `foo::x`는 멤버 루트로 걷는다.
        let mut d_mod = Module::new(PathBuf::from("d/lib.rs"), true, true);
        d_mod.items.insert("x".to_string());
        t.modules
            .get_mut("foo")
            .unwrap()
            .items
            .insert("x".to_string());
        t.modules.insert("d".to_string(), d_mod);
        assert_eq!(
            t.resolve("d", &["foo".into(), "x".into()], &map),
            Some("foo::x".to_string())
        );
    }

    #[test]
    fn single_segment_member_alias_resolves_to_root() {
        // `use real_lib;` 같은 단일 세그먼트 멤버 별칭 — 나머지가
        // 비어 walk가 루트 그 자체를 돌려야 한다.
        let t = tree();
        let map = dep_scope("real_lib", "real_lib", true);
        assert_eq!(
            t.resolve("c", &["real_lib".into()], &map),
            Some("real_lib".to_string())
        );
        // 외부 단일 세그먼트는 크레이트 정점으로 붕괴한다.
        let ext = dep_scope("serde", "serde", false);
        assert_eq!(
            t.resolve("c", &["serde".into()], &ext),
            Some("serde".to_string())
        );
    }

    /// 크레이트 c 스코프에 별칭 하나를 심은 dep 표를 만든다.
    fn dep_scope(alias: &str, vertex: &str, member: bool) -> DepCrates {
        let mut d = DepCrates::new();
        d.insert(
            "c".to_string(),
            alias.to_string(),
            DepTarget {
                vertex: vertex.to_string(),
                member,
            },
        );
        d
    }

    #[test]
    fn resolve_collapses_external_to_crate_vertex() {
        let t = tree();
        // 외부 크레이트 경로는 크레이트 정점으로 붕괴한다 — 안은 못 본다.
        let dep_map = dep_scope("serde", "serde", false);
        assert_eq!(
            t.resolve("c", &["serde".into(), "de".into()], &dep_map),
            Some("serde".to_string())
        );
        // rename된 의존은 코드상 이름으로 들어와 정점 이름으로 나간다.
        let renamed = dep_scope("foo", "real_pkg", false);
        assert_eq!(
            t.resolve("c", &["foo".into(), "x".into()], &renamed),
            Some("real_pkg".to_string())
        );
        // 빈 dep 표를 넘기면 옛 동작 — 미해석.
        assert_eq!(
            t.resolve("c", &["serde".into(), "de".into()], &DepCrates::new()),
            None
        );
        // 아무 것도 아닌 이름은 여전히 미해석.
        assert_eq!(t.resolve("c", &["nope".into()], &DepCrates::new()), None);
    }

    #[test]
    fn dep_alias_is_scoped_per_declaring_crate() {
        // 같은 별칭을 다른 크레이트가 다른 패키지에 물릴 수 있다 —
        // 스코프 밖 크레이트에서 쓴 별칭은 미해석이어야 한다.
        let mut t = tree();
        t.modules.insert(
            "other".to_string(),
            Module::new(PathBuf::from("o.rs"), true, true),
        );
        let map = dep_scope("foo", "real_pkg", false);
        // c는 foo를 선언했고 other는 안 했다 — other에서 foo는 미지다.
        assert_eq!(t.resolve("other", &["foo".into(), "x".into()], &map), None);
        assert_eq!(
            t.resolve("c", &["foo".into(), "x".into()], &map),
            Some("real_pkg".to_string())
        );
    }

    #[test]
    fn member_alias_walks_into_real_root() {
        // 멤버 별칭은 붕괴하지 않고 멤버의 실제 루트 정점부터 걷는다 —
        // 패키지 이름과 lib 타깃 이름이 다를 수 있기 때문이다.
        let mut t = tree();
        t.modules.insert(
            "real_lib".to_string(),
            Module::new(PathBuf::from("lib.rs"), true, true),
        );
        t.modules
            .get_mut("real_lib")
            .unwrap()
            .items
            .insert("f".to_string());
        let map = dep_scope("alias", "real_lib", true);
        assert_eq!(
            t.resolve("c", &["alias".into(), "f".into()], &map),
            Some("real_lib::f".to_string())
        );
        // 멤버 정점은 external이 아니다 — 임포트 후속 해석도 걷는다.
        assert!(!map.external.contains("real_lib"));
    }

    #[test]
    fn renamed_external_import_resolves_to_vertex() {
        // `use foo::make` (foo → real_pkg)가 만든 임포트의 후속 경로는
        // real_pkg 정점으로 붕괴한다 — 별칭 foo는 스코프 키이지
        // 정점 이름이 아니다.
        let mut t = tree();
        let map = dep_scope("foo", "real_pkg", false);
        // use 해석 자체 — 별칭이 정점 이름으로 붕괴한다.
        assert_eq!(
            t.resolve("c", &["foo".into(), "make".into()], &map),
            Some("real_pkg".to_string())
        );
        // 임포트 `make → real_pkg`를 심고 `make::x`를 해석한다 —
        // 임포트가 가리키는 외부 정점이 external 집합에 있어야 한다.
        t.modules
            .get_mut("c")
            .unwrap()
            .imports
            .insert("make".to_string(), bound("real_pkg", &Ns::ALL));
        assert_eq!(
            t.resolve("c", &["make".into(), "x".into()], &map),
            Some("real_pkg".to_string())
        );
    }

    #[test]
    fn mod_file_resolution() {
        let dir = std::env::temp_dir().join(format!("rg-modfile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("x.rs"), "pub fn a(){}").unwrap();
        std::fs::create_dir_all(dir.join("y")).unwrap();
        std::fs::write(dir.join("y").join("mod.rs"), "").unwrap();
        assert!(mod_file(&dir, "x", None).is_some());
        assert!(mod_file(&dir, "y", None).is_some());
        assert!(mod_file(&dir, "missing", None).is_none());
        // #[path] 우선.
        std::fs::write(dir.join("elsewhere.rs"), "").unwrap();
        assert!(mod_file(&dir, "z", Some("elsewhere.rs")).is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cfg_of_extracts_condition_tokens() {
        // #[cfg(test)]와 #[cfg(feature = "x")]는 조건 토큰 그대로 나온다.
        let item: syn::Item = syn::parse_str("#[cfg(test)] fn f() {}").unwrap();
        let attrs = match &item {
            syn::Item::Fn(f) => &f.attrs,
            _ => unreachable!(),
        };
        assert_eq!(cfg_of(attrs), Some("test".to_string()));
        let item: syn::Item =
            syn::parse_str("#[cfg(all(unix, feature = \"x\"))] #[cfg(debug_assertions)] fn f() {}")
                .unwrap();
        let attrs = match &item {
            syn::Item::Fn(f) => &f.attrs,
            _ => unreachable!(),
        };
        // 여러 cfg는 전부 성립해야 한다 — all()로 합성해 한 필드에 담는다.
        assert_eq!(
            cfg_of(attrs),
            Some("all(all (unix , feature = \"x\") , debug_assertions)".to_string())
        );
    }

    #[test]
    fn module_dir_rules() {
        // lib.rs/main.rs/mod.rs는 자기 디렉터리, 그 외는 stem/ 아래.
        assert_eq!(module_dir(Path::new("src/lib.rs")), PathBuf::from("src"));
        assert_eq!(module_dir(Path::new("src/a.rs")), PathBuf::from("src/a"));
        assert_eq!(
            module_dir(Path::new("src/a/mod.rs")),
            PathBuf::from("src/a")
        );
    }
}
