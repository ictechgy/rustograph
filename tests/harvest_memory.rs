//! 수확이 함수 본문 AST를 복제하지 않는지 힙 할당 바이트로 확인한다.
//!
//! 파싱한 AST 아레나는 수확 내내 살아 있다(누수시켜 'static으로 빌린다). 본문 방문용으로
//! 문장·표현식을 다시 복제하면 수확 피크가 그만큼(impl 메서드는 메서드와 본문을 두 번) 커진다 —
//! 큰 워크스페이스에서 피크가 AST의 약 두 배가 됐다. 이 테스트 바이너리만 전역 할당자를 계수기로
//! 바꾸고, 테스트도 하나뿐이라 다른 테스트 스레드의 할당이 섞이지 않는다.

use rustograph::{cargo_meta, source};
use std::alloc::{GlobalAlloc, Layout, System};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// 살아 있는 힙 바이트(요청 크기 기준)와 그 최댓값을 세는 할당자 — 실제 할당은 System이 한다.
struct CountingAllocator;

/// 지금 살아 있는 할당 바이트.
static LIVE_BYTES: AtomicUsize = AtomicUsize::new(0);
/// 마지막으로 재설정한 뒤 LIVE_BYTES가 닿은 최댓값.
static PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

/// 할당이 늘어난 만큼 더하고 최댓값을 갱신한다.
fn grow(bytes: usize) {
    let live = LIVE_BYTES.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK_BYTES.fetch_max(live, Ordering::Relaxed);
}

/// 할당이 줄어든 만큼 뺀다.
fn shrink(bytes: usize) {
    LIVE_BYTES.fetch_sub(bytes, Ordering::Relaxed);
}

// SAFETY: 모든 호출을 같은 인자로 System에 넘기고 성공한 할당의 크기만 센다 — 메모리 계약은
// System이 그대로 지킨다.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: 호출자가 GlobalAlloc::alloc의 계약(크기가 0이 아닌 layout)을 지킨다.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: alloc과 같다.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            grow(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: ptr은 이 할당자(곧 System)가 같은 layout으로 내준 블록이다.
        unsafe { System.dealloc(ptr, layout) };
        shrink(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: ptr은 System이 layout으로 내준 블록이고, new_size의 계약은 호출자가 지킨다.
        let new_ptr = unsafe { System.realloc(ptr, layout, new_size) };
        if !new_ptr.is_null() {
            if new_size >= layout.size() {
                grow(new_size - layout.size());
            } else {
                shrink(layout.size() - new_size);
            }
        }
        new_ptr
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// 워크스페이스 하나의 크기 — 모듈 파일마다 자유 함수·impl 메서드·트레이트 기본 메서드·
/// const 초기식을 둔다(본문을 복제하던 네 경로). 파일을 여럿으로 나눠 한 파일을 파싱하는 동안의
/// 토큰 스트림이 측정을 가리지 않게 한다.
const MODULES: usize = 20;
const FNS_PER_MODULE: usize = 8;
const STATEMENTS_PER_BODY: usize = 30;

/// 아이템 구조는 같고 본문의 문장 수만 다른 워크스페이스를 만든다. 문장은 매개변수만 쓰는
/// 산술이라 간선·미해석 경로·limitation을 만들지 않는다 — 두 워크스페이스의 그래프 문서가 같아,
/// 할당 차이가 본문 AST와 그 처리에서만 나온다.
fn workspace(tag: &str, statements: usize) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("rg-body-heap-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"bodyheap\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let body: String = (0..statements)
        .map(|i| format!("x = x * {i} + 1;\n"))
        .collect();
    let mut lib = String::new();
    for m in 0..MODULES {
        lib.push_str(&format!("pub mod m{m};\n"));
        std::fs::write(dir.join(format!("src/m{m}.rs")), module_source(&body)).unwrap();
    }
    std::fs::write(dir.join("src/lib.rs"), lib).unwrap();
    dir
}

/// 모듈 파일 하나 — 같은 본문을 네 종류의 본문 아이템에 싣는다.
fn module_source(body: &str) -> String {
    let mut src = String::from("pub struct S;\nimpl S {\n");
    for f in 0..FNS_PER_MODULE {
        src.push_str(&format!(
            "pub fn m{f}(&self, mut x: u32) -> u32 {{ {body} x }}\n"
        ));
    }
    src.push_str("}\npub trait T {\n");
    for f in 0..2 {
        src.push_str(&format!(
            "fn d{f}(&self, mut x: u32) -> u32 {{ {body} x }}\n"
        ));
    }
    src.push_str("}\n");
    for f in 0..FNS_PER_MODULE {
        src.push_str(&format!("pub fn f{f}(mut x: u32) -> u32 {{ {body} x }}\n"));
    }
    for f in 0..2 {
        src.push_str(&format!(
            "pub const C{f}: u32 = {{ let mut x = 1; {body} x }};\n"
        ));
    }
    src
}

/// `run`이 도는 동안 더 늘어난 힙의 최댓값(peak)과, 끝난 뒤 결과값을 쥔 채 남은 힙(kept)을 잰다.
fn measure<T>(run: impl FnOnce() -> T) -> (usize, usize) {
    let before = LIVE_BYTES.load(Ordering::Relaxed);
    PEAK_BYTES.store(before, Ordering::Relaxed);
    let out = run();
    let peak = PEAK_BYTES.load(Ordering::Relaxed) - before;
    let kept = LIVE_BYTES.load(Ordering::Relaxed).saturating_sub(before);
    drop(out);
    (peak, kept)
}

/// 심볼 레벨 수확 한 번 — 메타데이터는 밖에서 읽어 `cargo metadata` 출력이 측정에 섞이지 않는다.
fn harvest(dir: &Path, meta: &cargo_meta::Metadata) -> rustograph::graph::Document {
    let opts = source::Options {
        symbol_level: true,
        ..Default::default()
    };
    source::load_with(dir, meta, &opts).expect("synthetic workspace harvest failed")
}

/// 본문 크기에 비례하는 수확 중 일시 할당이 본문 AST의 4분의 1보다 작아야 한다. 아레나는 누수돼
/// 수확 뒤에도 남으므로 두 워크스페이스의 남은 힙 차이가 본문 AST 바이트고, (피크 − 남은 힙)의
/// 차이가 본문 때문에 수확 중에만 쓴 바이트다. 본문을 복제하던 수확은 본문 AST의 1.4배쯤을 더 썼다
/// (impl 메서드는 메서드 통째와 본문 표현식을 두 번 복제했다).
#[test]
fn harvest_borrows_function_bodies_instead_of_copying_them() {
    let empty = workspace("empty", 0);
    let full = workspace("full", STATEMENTS_PER_BODY);
    let empty_meta = cargo_meta::load(&empty).expect("cargo metadata for the empty workspace");
    let full_meta = cargo_meta::load(&full).expect("cargo metadata for the full workspace");
    // 처음 한 번만 만드는 전역 상태(툴체인·cfg 캐시 등)가 첫 측정에만 남지 않게 먼저 한 번 돈다.
    drop(harvest(&empty, &empty_meta));
    let (empty_peak, empty_kept) = measure(|| harvest(&empty, &empty_meta));
    let (full_peak, full_kept) = measure(|| harvest(&full, &full_meta));
    let _ = std::fs::remove_dir_all(&empty);
    let _ = std::fs::remove_dir_all(&full);

    let body_ast = full_kept
        .checked_sub(empty_kept)
        .expect("bodies must add retained AST bytes");
    let transient = (full_peak - full_kept).saturating_sub(empty_peak - empty_kept);
    // 입력이 실제로 큰 본문을 담았는지 — 작으면 비율이 잡음에 묻힌다.
    assert!(
        body_ast > 4 << 20,
        "synthetic bodies are too small to measure: {body_ast} bytes"
    );
    assert!(
        transient * 4 < body_ast,
        "harvest used {transient} extra transient heap bytes for {body_ast} bytes of body AST \
         (peak {full_peak} vs {empty_peak}, kept {full_kept} vs {empty_kept}); bodies must be \
         visited in place in the AST arena, not copied"
    );
}
