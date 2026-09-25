' 一键启动媒体库主程序：GUI 子系统 exe，不会产生任何控制台窗口。
' 用法：双击本文件。若提示 exe 不存在，请先运行一次 start.bat 完成构建。
CreateObject("Wscript.Shell").Run """G:\AI agent\media-manager\src-tauri\target\debug\media-manager.exe""", 1, False
