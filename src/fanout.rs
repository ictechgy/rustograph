//! 수신자 타입을 모르는 메서드 호출(`x.m()`)의 이름 팬아웃 정책 — 후보를 고르는 규칙.
//!
//! syn 수확([`crate::harvest`])과 의미 해석의 폴백(`sem`)이 같은 규칙으로 후보를 고르게 한 곳에 둔다.
//! 두 경로의 규칙이 갈라지면 같은 호출이 수확 모드에 따라 다른 간선을 낸다.
//!
//! 후보는 같은 이름의 워크스페이스 메서드 정점이다. 고유 메서드(`T::m`)와 트레이트 선언 메서드
//! (`Tr::m`)는 호출 크레이트와 그 전이 의존 폐포의 크레이트에서만 고른다. 폐포 밖 크레이트의
//! 타입·트레이트는 호출 크레이트가 이름 붙이거나 값으로 얻을 수 없어 정적으로 호출할 수 없다 — 범위를
//! 줄여도 "살아 있는 쪽으로 틀린다"는 계약은 그대로다. 트레이트 impl 메서드(`T::<Tr>::m`)는
//! dyn·제네릭 디스패치로 의존 없이도 닿으므로(호출 크레이트에 의존하는 크레이트의 impl) 워크스페이스
//! 전체에서 고른다.

use std::collections::{BTreeMap, BTreeSet};

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
}

/// 같은 이름 메서드 `ids` 중 이 호출이 닿을 수 있는 후보를 고른다. `visible`은 호출 크레이트의
/// [`Reach`] 항목이고, None이면 좁히지 않는다(옛 동작 — 라이브러리 호출자가 준 색인 그대로).
pub(crate) fn select<'a>(ids: &'a [String], visible: Option<&BTreeSet<String>>) -> Fanned<'a> {
    let picked: Vec<&str> = ids
        .iter()
        .map(String::as_str)
        .filter(|id| in_reach(id, visible))
        .collect();
    if picked.is_empty() {
        Fanned::OutOfReach
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
}
