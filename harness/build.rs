// Links build/libevmvx.a, produced by scripts/build-lib.sh, and the Vx task
// runtime (libvx_std_core) that evmvx_block_run spawns its workers on.
fn main() {
    let dir = std::env::var("EVMVX_LIB_DIR").unwrap_or_else(|_| {
        format!("{}/../build", std::env::var("CARGO_MANIFEST_DIR").unwrap())
    });
    println!("cargo:rustc-link-search=native={dir}");
    println!("cargo:rustc-link-lib=static=evmvx");
    println!("cargo:rerun-if-changed={dir}/libevmvx.a");

    // The runtime ships in the release's lib/, beside bin/vxc.
    let vx_lib = std::env::var("VX_LIB_DIR").unwrap_or_else(|_| {
        let vxc = std::env::var("VXC").expect("set VX_LIB_DIR, or VXC to the Vx release's bin/vxc");
        let bin = std::path::Path::new(&vxc).parent().expect("VXC has no parent directory");
        bin.join("../lib").display().to_string()
    });
    println!("cargo:rustc-link-search=native={vx_lib}");
    println!("cargo:rustc-link-lib=dylib=vx_std_core");
    println!("cargo:rustc-link-arg=-Wl,-rpath,{vx_lib}");
    println!("cargo:rerun-if-env-changed=VX_LIB_DIR");
    println!("cargo:rerun-if-env-changed=VXC");
}
