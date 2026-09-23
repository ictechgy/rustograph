//! OUT_DIR 산출물 fixture — load_out_dirs_from_check 없이는 include!가
//! 해석되지 않는다.
fn main() {
    let out = std::env::var("OUT_DIR").expect("OUT_DIR is set for build scripts");
    std::fs::write(
        format!("{out}/built_defs.rs"),
        "pub const BUILT_ANSWER: u32 = 5;\npub fn built_answer() -> u32 { 5 }\n",
    )
    .expect("write generated defs");
    std::fs::write(
        format!("{out}/root_defs.rs"),
        "pub const ROOT_ANSWER: u32 = 6;\n",
    )
    .expect("write root defs");
}
