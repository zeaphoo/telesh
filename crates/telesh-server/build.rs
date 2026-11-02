use std::path::Path;

fn main() {
    // `server-ui/dist` 由 rust-embed 在编译期读入。rust-embed 要求目录存在，
    // 否则直接编译失败，所以干净检出时先建一个空目录：debug 下面板会回 404，
    // 不影响测试。release 构建必须带真实前端，缺 index.html 直接失败，
    // 避免打出一个「dashboard 404」的 server 包。
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../server-ui/dist");
    let index = dist.join("index.html");

    if !index.is_file() {
        let profile = std::env::var("PROFILE").unwrap_or_default();
        // 只想要 CLI / 服务、不关心面板的源码构建可以显式放行：
        //   TELESH_ALLOW_EMPTY_UI=1 cargo build --release
        let allow_empty = std::env::var_os("TELESH_ALLOW_EMPTY_UI").is_some();
        if profile == "release" && !allow_empty {
            panic!(
                "server-ui/dist/index.html 不存在：release 构建必须嵌入前端。\n\
                 先执行 `./scripts/build-ui.sh`（或 `cd server-ui && pnpm build`）再编译。"
            );
        }
        std::fs::create_dir_all(&dist).expect("create server-ui/dist");
    }

    println!("cargo:rerun-if-changed={}", dist.display());
    println!("cargo:rerun-if-changed={}", index.display());
}
