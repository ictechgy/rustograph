mod extra;

use fixture_core::Greet;
use fixture_core::Used as Renamed;
use fixture_core::inline::*;
#[cfg(feature = "never")]
use fixture_core::cfg_gated;

fn main() {
    let u = Renamed { v: 1 };
    u.greet();
    let _quad = u.quadrupled();
    extra::run();
    let _c = fixture_core::Color::Red;
    println!("{}", fixture_core::entry());
    println!("{}", inline_fn());
    let _via_macro = fixture_core::emit_helper!();
    let _d = fixture_core::dyn_dispatch(&u);
    let _g = fixture_core::generic_dispatch(&u);
}
