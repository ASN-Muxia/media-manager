fn main() {
    // 前端产物改为**运行时**读取（见 src/assets.rs）：这里把 dist 的绝对路径在编译期
    // 写进环境变量 MM_DIST_DIR，运行时就近直读磁盘，改前端只需 `vite build`（亚秒），
    // 不必再重编这个 crate、更不必重新链接 exe。
    //
    // 用绝对路径而不是“相对 exe 推”：exe 在 target/<profile>/ 下，层级随构建方式
    // 变化，打包分发后必然算错。分发到别的机器时这个路径不存在，assets.rs 会自动
    // 回退到编译期嵌入的那份资源，分发包行为与改动前完全一致。
    //
    // ⚠️ 这里**刻意不再**输出 `rerun-if-changed=../dist`。
    // 历史：早先为了让“改了前端要重编进 exe”成立，补过这条监控；代价是每次改一行
    // CSS/TS 都要重编 media-manager 并重新链接 17MB 的 exe（实测 41.6 秒），而启动器
    // 是隐藏窗口跑的，用户看到的就是“点了没反应”。现在 dist 由运行时读取，改前端
    // 本来就不需要重编——这条监控只会把 41.6 秒白白加回来，故删除。
    //
    // 注意：一旦本脚本输出了任何 rerun-if-changed，Cargo 就**只**按这些路径判定是否
    // 重跑（这就是当初必须补 dist 的原因）。删掉它之后，重跑判据只剩 tauri_build 自己
    // 声明的那两条（tauri.conf.json / capabilities），这正是我们想要的：改前端不触发重编。
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let dist = root.parent().unwrap().join("dist");
    println!("cargo:rustc-env=MM_DIST_DIR={}", dist.display());
    tauri_build::build()
}
