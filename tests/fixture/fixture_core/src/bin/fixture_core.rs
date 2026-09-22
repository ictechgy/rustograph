//! fixture_core와 같은 이름의 bin — 루트 모듈이 extra_files로 합쳐진다.
//! 이 본문의 선언 파일은 lib.rs가 아니라 이 파일이어야 semantic 엔진의
//! 소스 정체 검사가 본문을 올바른 소스에 맞춘다.

use fixture_core::{Greet, Used};

fn main() {
    let u = Used { v: 3 };
    u.greet();
}
