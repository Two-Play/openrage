fn main() {
    // The logo as the program icon in Explorer.
    if std::env::var("CARGO_CFG_TARGET_OS").is_ok_and(|os| os == "windows") {
        winresource::WindowsResource::new()
            .set_icon("../../packaging/openrage.ico")
            .compile()
            .expect("Windows resources must compile");
    }
}
