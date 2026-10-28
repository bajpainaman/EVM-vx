// Links build/libevmvx.a, produced by scripts/build-lib.sh.
fn main() {
    let dir = std::env::var("EVMVX_LIB_DIR").unwrap_or_else(|_| {
        format!("{}/../build", std::env::var("CARGO_MANIFEST_DIR").unwrap())
    });
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static=evmvx");
    println!("cargo:rerun-if-changed={dir}/libevmvx.a");
}
