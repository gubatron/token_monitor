//! Compiles the Slint sources. Each workstream writes its own generated file
//! (`kit.rs` for ws-ui-kit, `settings.rs` for ws-ui-settings) so they never
//! clash on the single `include_modules!` slot.

use std::path::PathBuf;

fn compile(entry: &str, output: &str) {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".to_owned());
    let out = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo")).join(output);
    slint_build::compile_with_output_path(entry, out, config)
        .unwrap_or_else(|e| panic!("{entry} compiles: {e}"));
}

fn main() {
    compile("ui/settings/settings.slint", "settings.rs");
    println!("cargo:rerun-if-changed=ui/settings");
    // UI kit: components, gallery and snapshot hosts. Panels and settings
    // import components from `ui/components/kit.slint`.
    compile("ui/components/entry.slint", "kit.rs");
    println!("cargo:rerun-if-changed=ui/components");
    println!("cargo:rerun-if-changed=ui/assets");
}
