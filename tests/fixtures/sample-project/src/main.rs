//! Sample Rust fixture: binary entry.

mod utils;

use sample_project::{forge_config, unicorn_marker_fn, GoldConfig};

fn main() {
    let cfg: GoldConfig = forge_config("demo");
    println!("{}", unicorn_marker_fn(&cfg));
    let spin = utils::helper_spinner(cfg.depth);
    println!("spin={spin}");
}
