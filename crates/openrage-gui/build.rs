fn main() {
    // Debug builds keep element names and types, which the UI tests use to
    // find elements (Slint's testing backend); release builds leave them out.
    let debug = std::env::var("PROFILE").is_ok_and(|p| p == "debug");
    let config = slint_build::CompilerConfiguration::new().with_debug_info(debug);
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint UI must compile");
}
