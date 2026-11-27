use std::env;

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    // Clap's derived command tree can exceed the default Windows stack in
    // debug builds. Reserve extra address space for the lpcli binary and let
    // Windows commit stack pages as needed.
    const STACK_RESERVE: usize = 4 * 1024 * 1024;
    let argument = match env::var("CARGO_CFG_TARGET_ENV").as_deref() {
        Ok("msvc") => format!("/STACK:{STACK_RESERVE}"),
        Ok("gnu") => format!("-Wl,--stack,{STACK_RESERVE}"),
        _ => return,
    };
    println!("cargo::rustc-link-arg-bin=lpcli={argument}");
}
