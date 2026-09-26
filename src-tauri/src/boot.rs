//! 启动自愈：WebView2 缓存清理 + 前端加载看门狗。
//!
//! 背景（问题 1 的根因）：Windows 上 Tauri 把 WebView2 的用户数据目录固定在
//! `%LOCALAPPDATA%\<identifier>`，WebView2 会在其下建 `EBWebView`。当里面的 GPU
//! 持久缓存（`GPUPersistentCache\DawnGraphiteCache`）因为进程被强杀或杀毒软件
//! 扫描而损坏/被占用时，WebView2 的 GPU 子进程会崩溃重启，最后打出
//! `FATAL: GPU process isn't usable`，页面永远渲染不出来——表现就是"点了启动
//! 脚本没反应"：窗口标题在，内容全白，点哪都不动。
//!
//! 对策分三层，都是"宁可白做，不要漏做"：
//!   1. 每次启动、在创建任何 WebView 之前，删掉已知可重建的 GPU / 着色器缓存
//!      （这时还没有文件被本进程占用，删除不会失败）；
//!   2. 上一次启动若确认没起来，本次启动把整个 `EBWebView` **改名**挪走再删，
//!      并且改用软件渲染（`--disable-gpu`）绕开 GPU 子进程。
//!      用改名而不是直接删，是因为上一个进程可能还占着文件——直接删会失败，
//!      改名则立刻生效，新进程随即建一套全新目录，不受残骸影响；
//!   3. 看门狗：前端在时限内没上报就绪，就写标记并静默重启一次自愈；
//!      自愈后仍起不来，才弹窗把情况和出路告诉用户。
//!
//! 自动重启只允许一次，判据是"这次启动是不是已经被自愈拉起来的"——用一个
//! 标记文件当唯一事实来源，而不是计数或环境变量：重启是异步的，父子进程会
//! 短暂并存，计数会被彼此覆盖，环境变量也传不下去（都实测踩过），只有文件
//! 状态是确定性的。

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::db;

/// 自愈子进程标记：由父进程显式 spawn 时传入，标明"你是自愈轮次"。
const RETRY_ENV: &str = "MM_WEBVIEW_RETRY";

/// WebView2 额外启动参数（兜底那次启动用来关掉 GPU 进程）。
const BROWSER_ARGS_ENV: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

/// 重置标记（放在 WebView 数据目录旁边）：下次启动要彻底重置缓存。
const RESET_FLAG: &str = "webview_reset.flag";

/// 已自愈标记：本次启动就是被自愈拉起来的那一次，不该再重启一次。
const RESET_DONE: &str = "webview_reset.done";

/// 前端就绪标记（由 `frontend_ready` 命令写入）。
pub const READY_MARKER: &str = "frontend_ready.txt";

/// 缓存清理的日志文件。
const BOOT_LOG: &str = "webview_boot.log";

/// 看门狗等待前端就绪的时限。
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// 看门狗轮询间隔。
const POLL_INTERVAL: Duration = Duration::from_millis(500);

/// WebView 数据根：`%LOCALAPPDATA%\<identifier>`（与 Tauri 在 Windows 上的取值一致）。
pub fn webview_root(identifier: &str) -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Local"))
        })?;
    Some(base.join(identifier))
}

/// WebView2 真正写数据的目录（WebView2 自己会在用户数据根下建这一层）。
fn webview_dir(root: &Path) -> PathBuf {
    root.join("EBWebView")
}

/// 启动前准备：清缓存（必要时彻底重置）。
///
/// 必须在 `tauri::Builder::run` 之前调用——那时还没有任何 WebView / GPU 子进程
/// 被拉起来，缓存文件不会被自己占用。
pub fn prepare(identifier: &str) -> String {
    let Some(root) = webview_root(identifier) else {
        return "跳过 WebView 缓存清理：取不到 LOCALAPPDATA".to_string();
    };
    let flag = root.join(RESET_FLAG);
    let done = root.join(RESET_DONE);
    // flag = 上次失败、这次要重置；done = 这次就是被自愈拉起来的那一次。
    let needs_reset = flag.exists() || done.exists();
    let seen = format!("进入时 flag={} done={}", flag.exists(), done.exists());
    let mut report: Vec<String> = if needs_reset {
        let mut r = vec!["上次启动前端未就绪 → 彻底重置 WebView 数据目录".to_string()];
        // 自己是上一次失败后被拉起来的，上一个进程可能还没退干净。等一下再动
        // 目录，否则改名会被"拒绝访问"顶回来（实测踩过）；等 3 秒基本够父进程
        // 走完退出流程。
        std::thread::sleep(Duration::from_millis(3000));
        r.extend(reset_webview_dir(&root));
        // 常规清理没救回来，这一轮连 GPU 进程一起绕开：改用软件渲染，
        // 至少先把界面显示出来。
        std::env::set_var(BROWSER_ARGS_ENV, "--disable-gpu");
        r.push("本次改用软件渲染（--disable-gpu）".to_string());
        r
    } else {
        clean_volatile_caches(&root)
    };
    // 消费标记：flag 改名成 done，让看门狗知道"这次已经是自愈轮次"。
    // 必须用**改名**而不是"删掉再写"——重启是异步的，偶尔会同时冒出两个
    // 子进程，中间那一瞬的空档会让另一个进程既看不到 flag 也看不到 done，
    // 于是又自愈一次。改名是原子操作，谁先谁后都能正确判定。
    if needs_reset {
        if done.exists() {
            // 已经是自愈轮次了：flag 只是重复记账（父进程先把 done 落盘再
            // spawn，所以两个标记会同时出现）。Windows 上 `rename` 不允许
            // 覆盖已存在的目标，直接改名会失败，这里删掉 flag 就行。
            let _ = std::fs::remove_file(&flag);
        } else if let Err(e) = std::fs::rename(&flag, &done) {
            report.push(format!("消费重置标记失败: {e}"));
        }
    } else {
        // 正常启动：不留历史痕迹。
        let _ = std::fs::remove_file(&flag);
    }
    report.push(seen);
    let line = report.join(" | ");
    append_boot_log(&root, &line);
    line
}

/// 常规清理：删掉会损坏但可重建的缓存。
fn clean_volatile_caches(root: &Path) -> Vec<String> {
    let dir = webview_dir(root);
    let mut report = Vec::new();
    if !dir.exists() {
        return report;
    }
    // 这些目录的共同点：坏了会让 GPU 子进程起不来（页面就此白屏），
    // 删掉后 WebView2 会自己重建，代价只是首次绘制慢一点。
    for name in ["GPUPersistentCache", "GrShaderCache", "ShaderCache"] {
        remove_dir_quiet(&dir.join(name), &mut report);
    }
    for name in [
        "GPUCache",
        "Code Cache",
        "Cache",
        "DawnGraphiteCache",
        "DawnWebGPUCache",
    ] {
        remove_dir_quiet(&dir.join("Default").join(name), &mut report);
    }
    report.extend(clean_stale_backups(root));
    report
}

/// 彻底重置：把 `EBWebView` 改名挪走再删。
fn reset_webview_dir(root: &Path) -> Vec<String> {
    let mut report = Vec::new();
    move_away_then_delete(&webview_dir(root), &mut report);
    report.extend(clean_stale_backups(root));
    report
}

/// 清掉上次改名留下的残骸（这些目录已经没人用了，删不掉下次再试）。
fn clean_stale_backups(root: &Path) -> Vec<String> {
    let mut report = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return report;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("EBWebView.bad-") {
            remove_dir_quiet(&entry.path(), &mut report);
        }
    }
    report
}

fn remove_dir_quiet(path: &Path, report: &mut Vec<String>) {
    if !path.exists() {
        return;
    }
    match std::fs::remove_dir_all(path) {
        Ok(()) => report.push(format!("已清理 {}", path.display())),
        // 上一个实例还在跑时会删不掉，属于预期内的失败，忽略即可。
        Err(e) => report.push(format!("清理 {} 失败（可忽略）: {e}", path.display())),
    }
}

fn move_away_then_delete(path: &Path, report: &mut Vec<String>) {
    if !path.exists() {
        return;
    }
    let base = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "EBWebView".to_string());
    let mut target = path.with_file_name(format!("{base}.bad-{}", db::now_epoch()));
    let mut i = 0;
    while target.exists() && i < 100 {
        i += 1;
        target = path.with_file_name(format!("{base}.bad-{}-{i}", db::now_epoch()));
    }
    match std::fs::rename(path, &target) {
        Ok(()) => {
            report.push(format!("已移走 {}", path.display()));
            remove_dir_quiet(&target, report);
        }
        Err(e) => report.push(format!("移走 {} 失败: {e}", path.display())),
    }
}

fn append_boot_log(root: &Path, line: &str) {
    if line.is_empty() {
        return;
    }
    let path = root.join(BOOT_LOG);
    let mut buf = std::fs::read_to_string(&path).unwrap_or_default();
    buf.push_str(&format!(
        "[{}] pid={} {line}\n",
        db::now_epoch(),
        std::process::id()
    ));
    if buf.len() > 64 * 1024 {
        let keep = buf.len() - 32 * 1024;
        buf = buf.split_off(keep);
    }
    let _ = std::fs::write(path, buf);
}

/// 启动前删掉上一次的就绪标记，避免看门狗把旧文件当成"这次起来了"。
pub fn clear_ready_marker(app: &AppHandle) {
    if let Ok(dir) = db::data_dir(app) {
        let _ = std::fs::remove_file(dir.join(READY_MARKER));
    }
}

/// 起看门狗：前端在时限内没上报就绪就自愈一次。
///
/// 存在的理由：清缓存只能防住"GPU 缓存坏了"这一类故障，别的白屏原因（资源
/// 缺失、WebView2 运行时异常）同样表现为"点了没反应"。与其让用户对着白窗
/// 发呆，不如自己发现问题、先重启自愈一次，实在不行再说清楚。
pub fn arm_watchdog(app: AppHandle, identifier: String) {
    std::thread::spawn(move || {
        let deadline = Instant::now() + READY_TIMEOUT;
        loop {
            std::thread::sleep(POLL_INTERVAL);
            if ready(&app) {
                // 这次起来了：清掉自愈痕迹，下次真出事还能再自愈一次。
                let _ = crate::commands::diag::append_log(&app, "前端已就绪，清除自愈标记");
                clear_recovery_marks(&identifier);
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        on_load_failed(app, identifier);
    });
}

fn ready(app: &AppHandle) -> bool {
    db::data_dir(app)
        .map(|d| d.join(READY_MARKER).exists())
        .unwrap_or(false)
}

fn on_load_failed(app: AppHandle, identifier: String) {
    let _ = crate::commands::diag::append_log(
        &app,
        &format!("看门狗：{READY_TIMEOUT:?} 内前端未上报就绪，界面可能白屏"),
    );

    let Some(root) = webview_root(&identifier) else {
        // 连数据目录都定位不到，没法记账，也就不能冒险重启。
        let _ = crate::commands::diag::append_log(&app, "取不到 LOCALAPPDATA，跳过自愈");
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&root) {
        let _ = crate::commands::diag::append_log(&app, &format!("创建 WebView 数据目录失败: {e}"));
        return;
    }
    if let Err(e) = write_flag(&root) {
        let _ = crate::commands::diag::append_log(&app, &format!("写重置标记失败: {e}"));
    }

    // 只有"普通启动"才自愈重启；`done` 或 `RETRY_ENV` 说明这次本来就是被自愈
    // 拉起来的，再重启就会变成来回重启的死循环。（文件和环境变量双保险：
    // 只靠文件，并发子进程可能在标记落盘前启动；只靠环境变量，`app.restart()`
    // 又传不下去。现在进程是我们自己 spawn 的，两者都能拿到。）
    let recovering = root.join(RESET_DONE).exists() || std::env::var(RETRY_ENV).is_ok();
    let _ = crate::commands::diag::append_log(
        &app,
        &format!("自愈判定: pid={} 自愈轮次={recovering}", std::process::id()),
    );
    if !recovering {
        // 先把"已自愈"落盘，再自己拉起下一个进程。
        // 不用 `app.restart()`：实测它会拉出不止一个子进程，而且退出时机不可控，
        // 子进程会抢在标记落盘/父进程释放目录之前动手，于是又自愈一轮。
        // 自己 spawn 则只有一个子进程，环境变量也一定传得过去。
        mark_recovered(&root);
        let spawned = std::env::current_exe().ok().and_then(|exe| {
            std::process::Command::new(exe)
                .env(RETRY_ENV, "1")
                .spawn()
                .ok()
        });
        let _ = crate::commands::diag::append_log(
            &app,
            &format!("自愈：拉起新进程 {}", spawned.is_some()),
        );
        if spawned.is_some() {
            app.exit(0);
            return;
        }
    }

    // 自愈过一次还是没起来：不再折腾，直接把情况和出路告诉用户。
    //
    // 同时把 flag / done 一并清掉：这两个标记都是"下一轮该怎么补救"的指令，
    // 留着会让用户手动重开的那一次被误判成自愈轮次，白白少一次自愈机会。
    // 清掉之后，下一次由用户主动发起的启动又是普通启动，还能再自愈一次。
    // （本进程此时只等用户点掉弹窗，不再读这两个标记，所以清它们是安全的。）
    clear_recovery_marks(&identifier);
    app.dialog()
        .message(
            "界面没能加载出来，已经重置过一次 WebView 缓存仍无改善。\n\
             请关闭本程序后重新打开；若反复出现，请先升级 WebView2 运行时。\n\
             详细日志见应用数据目录下的 frontend_error.log。",
        )
        .title("影视库启动异常")
        .kind(MessageDialogKind::Error)
        .blocking_show();
}

fn write_flag(root: &Path) -> Result<(), String> {
    std::fs::write(root.join(RESET_FLAG), format!("ts={}\n", db::now_epoch()))
        .map_err(|e| format!("写重置标记失败: {e}"))
}

/// 落下"已经自愈过"的标记，并顺手把重置标记也写上（两者通常一起出现）。
fn mark_recovered(root: &Path) {
    let _ = write_flag(root);
    let _ = std::fs::write(root.join(RESET_DONE), format!("ts={}\n", db::now_epoch()));
}

/// 前端起来了：把重置 / 自愈标记一并清掉，下次真出事还能再自愈一次。
fn clear_recovery_marks(identifier: &str) {
    let Some(root) = webview_root(identifier) else {
        return;
    };
    let _ = std::fs::remove_file(root.join(RESET_DONE));
    let _ = std::fs::remove_file(root.join(RESET_FLAG));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个临时根目录：`<temp>/mm-boot-<pid>-<seq>/com.example.media-manager`。
    fn temp_root(tag: &str) -> PathBuf {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir()
            .join(format!("mm-boot-{tag}-{}-{n}", std::process::id()))
            .join("com.example.media-manager")
    }

    #[test]
    fn webview_root_uses_local_appdata() {
        let root = webview_root("com.example.media-manager").expect("本机应有 LOCALAPPDATA");
        let name = root.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(name, "com.example.media-manager");
        assert_eq!(webview_dir(&root), root.join("EBWebView"));
    }

    #[test]
    fn volatile_caches_are_removed_but_profile_is_kept() {
        let root = temp_root("clean");
        let ebweb = root.join("EBWebView");
        std::fs::create_dir_all(ebweb.join("GPUPersistentCache").join("DawnGraphiteCache"))
            .expect("建测试目录");
        std::fs::create_dir_all(ebweb.join("Default").join("GPUCache")).expect("建测试目录");
        std::fs::write(ebweb.join("Local State"), "x").expect("写测试文件");

        let report = clean_volatile_caches(&root);

        assert!(
            !ebweb.join("GPUPersistentCache").exists(),
            "GPU 持久缓存应被清掉: {report:?}"
        );
        assert!(
            !ebweb.join("Default").join("GPUCache").exists(),
            "Default\\GPUCache 应被清掉: {report:?}"
        );
        // 常规清理只动缓存，不能把整个配置档端掉。
        assert!(
            ebweb.join("Local State").exists(),
            "非缓存文件不应被删: {report:?}"
        );

        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn reset_moves_the_whole_directory_aside() {
        let root = temp_root("reset");
        let ebweb = root.join("EBWebView");
        std::fs::create_dir_all(ebweb.join("GPUPersistentCache")).expect("建测试目录");

        let report = reset_webview_dir(&root);

        assert!(!ebweb.exists(), "EBWebView 应被整体移走: {report:?}");
        let leftovers: Vec<String> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with("EBWebView.bad-"))
            .collect();
        // 改名一定能成功；删不删得掉取决于系统有没有别的句柄，不强求。
        assert!(
            leftovers.len() <= 1,
            "最多留一个待清理残骸: {leftovers:?}"
        );

        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn prepare_consumes_flag_and_leaves_the_done_mark() {
        let root = temp_root("flag");
        let ebweb = root.join("EBWebView");
        std::fs::create_dir_all(&ebweb).expect("建测试目录");
        std::fs::write(root.join(RESET_FLAG), "ts=1\n").expect("写标记");

        // 复刻 prepare 的判定与消费动作：有 flag 或有 done 都是"自愈轮次"。
        let flag = root.join(RESET_FLAG);
        let done = root.join(RESET_DONE);
        let needs_reset = flag.exists() || done.exists();
        let report = if needs_reset {
            reset_webview_dir(&root)
        } else {
            clean_volatile_caches(&root)
        };
        if needs_reset {
            std::fs::rename(&flag, &done).expect("flag 改名成 done");
        } else {
            std::fs::remove_file(&flag).ok();
        }

        assert!(needs_reset, "有标记就该判定为自愈轮次");
        assert!(!ebweb.exists(), "自愈轮次必须彻底重置: {report:?}");
        assert!(!flag.exists(), "flag 用完必须删掉");
        // done 是本轮不再自动重启的唯一依据，必须留下。
        assert!(done.exists(), "done 必须留下，否则会来回重启");
        // 只有 done（没有 flag）时也仍然是自愈轮次。
        assert!(done.exists() && !flag.exists());

        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn flag_is_dropped_when_the_recovery_round_is_already_marked() {
        // 父进程先落 `done` 再 spawn，子进程启动时两个标记会同时在场。
        // Windows 的 rename 不允许覆盖已有目标，所以这条分支必须走"删 flag"。
        let root = temp_root("both");
        std::fs::create_dir_all(&root).expect("建测试目录");
        let flag = root.join(RESET_FLAG);
        let done = root.join(RESET_DONE);
        std::fs::write(&flag, "ts=1\n").expect("写 flag");
        std::fs::write(&done, "ts=1\n").expect("写 done");

        let needs_reset = flag.exists() || done.exists();
        assert!(needs_reset, "两个标记都在，当然是自愈轮次");
        done_cleanup(&flag, &done);

        assert!(!flag.exists(), "flag 必须被消费掉");
        assert!(done.exists(), "done 是本轮不再重启的依据，必须留下");

        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    /// 与 `prepare` 里的标记消费逻辑保持一致（测试无法直接调用 prepare，
    /// 因为它会 sleep 3 秒并改环境变量）。
    fn done_cleanup(flag: &Path, done: &Path) {
        if done.exists() {
            let _ = std::fs::remove_file(flag);
        } else {
            let _ = std::fs::rename(flag, done);
        }
    }
}
