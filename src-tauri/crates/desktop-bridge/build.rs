fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        println!("cargo:rerun-if-changed=src/claude/permission_selector.m");
        cc::Build::new()
            .file("src/claude/permission_selector.m")
            .flag("-fobjc-arc")
            .compile("claude_permission_selector");
        println!("cargo:rustc-link-lib=framework=AppKit");
        println!("cargo:rustc-link-lib=framework=ApplicationServices");
    }
}
