fn main() {
    // sidecar 的文件名带 target triple 后缀,运行时要用它去找二进制
    // (见 src/server.rs 的 `find_binary`)。这里把 build 脚本拿到的 TARGET 传进去。
    println!(
        "cargo:rustc-env=TELESH_TARGET_TRIPLE={}",
        std::env::var("TARGET").expect("TARGET is set by cargo")
    );
    println!("cargo:rerun-if-changed=tauri.conf.json");
    println!("cargo:rerun-if-changed=capabilities");
    println!("cargo:rerun-if-changed=icons");
    tauri_build::build()
}
