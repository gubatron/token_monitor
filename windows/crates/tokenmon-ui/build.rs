//! Compiles the Slint sources. Each workstream writes its own generated file
//! (`settings.rs` here) so they never clash on the single include slot.

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".to_owned());
    let out = std::path::PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR set by cargo"))
        .join("settings.rs");
    slint_build::compile_with_output_path("ui/settings/settings.slint", out, config)
        .expect("settings.slint compiles");
    println!("cargo:rerun-if-changed=ui/settings");
}
