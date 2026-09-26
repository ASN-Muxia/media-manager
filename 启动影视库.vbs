Option Explicit
' 一键启动媒体库主程序：GUI 子系统 exe，不会产生任何控制台窗口。
' 用法：双击本文件。若提示 exe 不存在，请先运行一次 start.bat 完成构建。
Dim shell, fso, exe, alt
exe = "G:\AI agent\media-manager\src-tauri\target\debug\media-manager.exe"
alt = "G:\AI agent\media-manager\src-tauri\target\release\media-manager.exe"
Set fso = CreateObject("Scripting.FileSystemObject")
If Not fso.FileExists(exe) Then
    If fso.FileExists(alt) Then
        exe = alt
    Else
        MsgBox "找不到主程序：" & vbCrLf & exe & vbCrLf & vbCrLf & _
               "请先双击 start.bat 完成一次构建，再点本文件启动。", 16, "影视库"
        WScript.Quit 1
    End If
End If
Set shell = CreateObject("Wscript.Shell")
shell.Run """" & exe & """", 1, False
