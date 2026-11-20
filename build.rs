fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        winresource::WindowsResource::new()
            .set_icon("assets/relay.ico")
            .set("ProductName", "Relay")
            .set("FileDescription", "Relay portable network utility")
            .compile()
            .expect("Windows resource compiler unavailable");
    }
    println!("cargo:rerun-if-changed=assets/relay.ico");
}
