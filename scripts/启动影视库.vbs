Option Explicit
' 启动影视库（双击即可，全程不弹命令行黑框）
'
' 与 start.bat 走**同一个入口**：同一份 exe、同一套构建规则。
' 这里**不再**自己挑 debug / release —— 那正是三个入口行为不一致的根源
' （以前本文件优先 debug，而直接双击的 exe 却是 release 目录下可能早已
'  过期的那份）。现在统一由 start.bat 决定（默认 release）。
'
' 流程：先看 exe 是否比前端产物新，是就直接启动（秒开）；
'      否则先用隐藏窗口跑一次 start.bat 重建，再启动。
Dim shell, fso, root, bat, exe, cmdExe, rc, Q, needFe, needRs

' 引号统一用 Chr(34) 拼：避免层层转义，也避免路径里的空格被截断
Q = Chr(34)
root = "G:\AI agent\media-manager"
bat  = root & "\start.bat"
exe  = root & "\src-tauri\target\release\media-manager.exe"

Set fso   = CreateObject("Scripting.FileSystemObject")
Set shell = CreateObject("Wscript.Shell")

If Not fso.FileExists(bat) Then
    MsgBox "找不到启动脚本：" & vbCrLf & bat, 16, "影视库"
    WScript.Quit 1
End If

' ---- 判断要不要构建：交给 need-build.mjs（与 start.bat 同一份判据）----
'
' [!] 这里**不再拿 dist 的 mtime 跟 exe 比**。
' 前端产物现在是**运行时**从 dist 目录读的（见 src-tauri/src/assets.rs），
' 「dist 比 exe 新」是**正常状态**，不是「要重建」的信号。
' 旧逻辑一旦发现 dist 新，就用隐藏窗口跑一次完整重建 ——
' 实测那是 41.6 秒（重编 crate + 链接 17MB exe），而窗口是隐藏的，
' 用户看到的就是「双击之后卡死半分钟」。那正是「启动非常慢」的根因。
'
' 现在只用 Rust 侧源码/清单的变化判定要不要重链；前端改动不再触发 cargo build。
cmdExe = shell.ExpandEnvironmentStrings("%COMSPEC%")

needFe = NeedBuild("--frontend")
needRs = NeedBuild("--rust --profile release")

If needFe Or needRs Then
    ' 构建前先清掉残留实例：
    '   ① exe 被占用会让链接失败或变慢（实测报「拒绝访问」）；
    '   ② 多实例抢 WebView2 数据目录会误触发「启动异常」弹窗。
    KillStale

    ' 1 = 正常窗口：让用户**看见**构建进度，不再像卡死。
    ' True = 等它结束并取退出码。
    ' MM_NO_PAUSE=1 必不可少：否则失败时停在 pause 上，等不到退出码。
    rc = shell.Run(Q & cmdExe & Q & " /c set MM_NO_PAUSE=1 && call " & Q & bat & Q, 1, True)
    If rc <> 0 Then
        MsgBox "构建或启动失败（退出码 " & rc & "）。" & vbCrLf & vbCrLf & _
               "请双击 start.bat 查看完整错误信息。", 16, "影视库"
        WScript.Quit rc
    End If

    ' start.bat 自己会拉起 exe；这里兜底确认一次
    If Not fso.FileExists(exe) Then
        MsgBox "构建结束但找不到可执行文件：" & vbCrLf & exe, 16, "影视库"
        WScript.Quit 1
    End If
    WScript.Quit 0
End If

' ---- 无需构建（绝大多数启动都走这条）：直接秒开 ----
shell.Run Q & exe & Q, 1, False
WScript.Quit 0
' ---- 辅助函数 ----
'
' NeedBuild：问 need-build.mjs 要不要构建（与 start.bat 用同一份判据，避免两处规则不一致）。
' 参数是透传给脚本的开关串；退出码 0 = 需要构建。
' 0 = 隐藏窗口；False = 不等它（只取结论）。脚本只输出结论、不做构建，所以很快。
Function NeedBuild(args)
    Dim node, mjs, rc
    NeedBuild = False
    node = NodeExe()
    If node = "" Then NeedBuild = True: Exit Function   ' 找不到 node 就保守地按需要构建处理
    mjs = root & "\mjs\dev\need-build.mjs"
    If Not fso.FileExists(mjs) Then NeedBuild = True: Exit Function
    rc = shell.Run(Q & node & Q & " " & Q & mjs & Q & " " & args, 0, True)
    If rc = 0 Then NeedBuild = True
End Function
'
' NodeExe：找 node.exe。优先环境变量 MM_NODE，其次本机已知路径。
Function NodeExe()
    Dim p
    NodeExe = ""
    p = shell.ExpandEnvironmentStrings("%MM_NODE%")
    If p <> "%MM_NODE%" And fso.FileExists(p) Then NodeExe = p: Exit Function
    p = "C:\Users\31506\.workbuddy\binaries\node\versions\22.22.2-6\node.exe"
    If fso.FileExists(p) Then NodeExe = p: Exit Function
    p = "C:\Users\31506\node\node-v22.23.3-win-x64\node.exe"
    If fso.FileExists(p) Then NodeExe = p: Exit Function
End Function
'
' KillStale：构建/启动前清掉残留的 media-manager.exe 进程。
' 为什么需要：
'   1. 残留实例占住 exe，链接会失败或变慢（实测报「拒绝访问」）；
'   2. 多实例抢同一个 WebView2 数据目录，会让看门狗误判「白屏」并弹错误框。
' 用 taskkill /F /IM，按镜像名杀，不需要 pid。失败无所谓（本来就没有）。
Sub KillStale()
    Dim rc
    rc = shell.Run(Q & shell.ExpandEnvironmentStrings("%COMSPEC%") & Q & " /c taskkill /F /IM media-manager.exe >nul 2>nul", 0, True)
End Sub
'
