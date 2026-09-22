//! rustograph 바이너리 진입점 — 실제 구현은 라이브러리에 있다.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut stdout = std::io::stdout();
    let mut stderr = std::io::stderr();
    ExitCode::from(rustograph::cli::run(&args, &mut stdout, &mut stderr) as u8)
}
