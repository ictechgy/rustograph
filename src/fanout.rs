//! 수신자 타입을 모르는 메서드 호출(`x.m()`)의 이름 팬아웃 정책 — 후보를 고르는 규칙과 폭 상한.
//!
//! syn 수확([`crate::harvest`])과 의미 해석의 폴백(`sem`)이 같은 규칙으로 후보를 고르게 한 곳에 둔다.
//! 두 경로의 규칙이 갈라지면 같은 호출이 수확 모드에 따라 다른 간선을 낸다.
//!
//! 후보는 같은 이름의 워크스페이스 메서드 정점이고, 두 단계로 좁힌다.
//! - 고유 메서드(`T::m`)와 트레이트 선언 메서드(`Tr::m`)는 호출 크레이트와 그 전이 의존 폐포의
//!   크레이트에서만 고른다. 폐포 밖 크레이트의 타입·트레이트는 호출 크레이트가 이름 붙이거나 값으로
//!   얻을 수 없어 정적으로 호출할 수 없다 — 범위를 줄여도 "살아 있는 쪽으로 틀린다"는 계약은 그대로다.
//!   트레이트 impl 메서드(`T::<Tr>::m`)는 dyn·제네릭 디스패치로 의존 없이도 닿으므로(호출 크레이트에
//!   의존하는 크레이트의 impl) 워크스페이스 전체에서 고른다.
//! - 좁힌 뒤에도 후보가 [`MAX_CANDIDATES`]를 넘는 호출은 간선을 달지 않고 센다. 호출 수와 같은 이름
//!   정의 수가 둘 다 코드 크기에 비례해, 상한이 없으면 추정 간선이 코드 크기의 제곱으로 는다.

use std::collections::{BTreeMap, BTreeSet};

/// 호출 하나가 펼칠 수 있는 후보 수의 상한이다.
///
/// 실제 대형 워크스페이스보다 넉넉하게 잡은 안전 상한이다. 정상 저장소에서는 걸리지 않고, 같은 이름
/// 정의가 워크스페이스 크기에 비례해 느는 병적 입력에서만 작동한다. 실측(2026-10, 의존 폐포로 좁힌 뒤
/// 호출 하나가 펼친 최대 후보 수): 이 저장소 5, rust-analyzer 크레이트 5개(`ra_ap_hir` 계열 0.0.349)
/// 119(`new`), 27개 전부(정점 1.6만, 간선 63만) 544(`fmt` — 트레이트 impl이라 전체에서 고른다). 그
/// 최대값의 7배를 넘는 2의 거듭제곱이다. 넘는 호출은 그 이름의 메서드를 살려 두지 못하므로 dead가
/// unreachable로 보고할 수 있다 — 수확이 limitation으로 알린다.
pub(crate) const MAX_CANDIDATES: usize = 4096;

/// 크레이트(타깃 루트 모듈 이름) → 그 크레이트 본문의 팬아웃이 고유·트레이트 선언 메서드를 고를 수
/// 있는 크레이트(자기 포함). 맵에 없는 크레이트(의존 그래프를 모를 때 등)는 좁히지 않는다.
pub(crate) type Reach = BTreeMap<String, BTreeSet<String>>;

/// 호출 하나의 팬아웃 판정이다.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Fanned<'a> {
    /// 이 후보들에 추정 간선을 단다.
    Candidates(Vec<&'a str>),
    /// 같은 이름 메서드는 있지만 전부 의존 폐포 밖 크레이트의 고유·트레이트 선언 메서드다.
    OutOfReach,
    /// 좁힌 후보가 [`MAX_CANDIDATES`]를 넘는다 — 간선 없이 센다.
    Capped,
}

/// 같은 이름 메서드 `ids` 중 이 호출이 닿을 수 있는 후보를 고른다. `visible`은 호출 크레이트의
/// [`Reach`] 항목이고, None이면 좁히지 않는다(옛 동작 — 라이브러리 호출자가 준 색인 그대로).
pub(crate) fn select<'a>(ids: &'a [String], visible: Option<&BTreeSet<String>>) -> Fanned<'a> {
    select_within(ids, visible, MAX_CANDIDATES)
}

/// [`select`]의 본체 — 상한을 인자로 받아 단위 테스트가 작은 값으로 경계를 본다.
fn select_within<'a>(
    ids: &'a [String],
    visible: Option<&BTreeSet<String>>,
    max: usize,
) -> Fanned<'a> {
    let picked: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| in_reach(id, visible))
        .collect();
    if picked.is_empty() {
        Fanned::OutOfReach
    } else if picked.len() > max {
        Fanned::Capped
    } else {
        Fanned::Candidates(picked)
    }
}

/// 메서드 정점 `id`를 이 호출의 후보로 삼을 수 있는가 — 트레이트 impl 메서드(ID에 `::<`)는 항상,
/// 그 밖의 메서드는 정점 ID의 첫 세그먼트(선언 크레이트)가 `visible`에 있을 때만이다. 고유 메서드는
/// 타입의 크레이트에만, 트레이트 선언 메서드는 트레이트의 크레이트에만 있으므로 첫 세그먼트가 곧 선언
/// 크레이트다.
fn in_reach(id: &str, visible: Option<&BTreeSet<String>>) -> bool {
    let Some(visible) = visible else {
        return true;
    };
    if id.contains("::<") {
        return true;
    }
    let declared_in = id.split("::").next().unwrap_or(id);
    visible.contains(declared_in)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 문자열 목록을 소유 ID 목록으로 — 표 테스트를 짧게 쓰기 위한 도우미.
    fn owned_ids(ids: &[&str]) -> Vec<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    /// 크레이트 이름 집합 — 표 테스트를 짧게 쓰기 위한 도우미.
    fn crate_set(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    /// 고유·트레이트 선언 메서드는 보이는 크레이트 것만, 트레이트 impl 메서드는 크레이트와 무관하게
    /// 고른다. 다 걸러지면 OutOfReach이고, `visible`이 없으면 좁히지 않는다.
    #[test]
    fn select_keeps_visible_crates_and_every_trait_impl() {
        let ids = owned_ids(&["a::S::get", "b::T::get", "b::Tr::get", "c::U::<Tr>::get"]);
        let visible = crate_set(&["a"]);
        assert_eq!(
            select(&ids, Some(&visible)),
            Fanned::Candidates(vec!["a::S::get", "c::U::<Tr>::get"])
        );
        assert_eq!(
            select(&ids, None),
            Fanned::Candidates(ids.iter().map(String::as_str).collect())
        );
        let hidden = owned_ids(&["b::T::get", "b::Tr::get"]);
        assert_eq!(select(&hidden, Some(&visible)), Fanned::OutOfReach);
        // 이름이 접두사인 다른 크레이트(`a`와 `ab`)는 같은 크레이트가 아니다.
        let prefix = owned_ids(&["ab::S::get"]);
        assert_eq!(select(&prefix, Some(&visible)), Fanned::OutOfReach);
    }

    /// 상한은 좁힌 뒤의 후보 수에 적용한다 — 정확히 상한이면 펼치고, 하나라도 넘으면 센다.
    #[test]
    fn select_caps_the_candidates_left_after_narrowing() {
        let ids = owned_ids(&["a::S::get", "a::T::get", "b::T::get", "b::U::get"]);
        let visible = crate_set(&["a"]);
        assert_eq!(
            select_within(&ids, Some(&visible), 2),
            Fanned::Candidates(vec!["a::S::get", "a::T::get"])
        );
        assert_eq!(select_within(&ids, Some(&visible), 1), Fanned::Capped);
        assert_eq!(select_within(&ids, None, 3), Fanned::Capped);
    }
}
