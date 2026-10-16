# 给外壳的某个窗口发一次真正的关闭请求（WM_CLOSE），用来验证「关窗契约」。
#
# 为什么需要它：窗口的 × 与 Alt+F4 走的是 Tauri 的 `CloseRequested` 事件，Playwright
# 关不掉宿主窗口、`capture-window.ps1` 也只会截图。要端到端验证「编辑当前 / 浏览器
# 编辑区 / 牌组选项」的关窗拦截，只能真的给那个窗口发一条 WM_CLOSE。
#
# 用法：
#   pwsh -File tauri/tools/close-window.ps1 -ProcessName anki-tauri -TitleLike "编辑当前"
#   pwsh -File tauri/tools/close-window.ps1 -ProcessName anki-tauri -All
#
# 退出码：0 = 至少给一个窗口发了关闭请求；1 = 没找到窗口（或发送失败）。

[CmdletBinding()]
param(
    [string]$ProcessName = "anki-tauri",
    [string]$TitleLike = "",
    [switch]$All,
    [int]$WaitSeconds = 10
)

$ErrorActionPreference = "Stop"

if (-not ("WinClose" -as [type])) {
    Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class WinClose {
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr SendMessage(IntPtr hwnd, uint msg, IntPtr wparam, IntPtr lparam);

    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lparam);

    /// 枚举某个进程的全部可见顶层窗口（按 z 序，最上面的在前）——与 capture-window.ps1 同一套。
    public static IntPtr[] FindWindows(uint pid, string titleLike) {
        var found = new List<IntPtr>();
        EnumWindows((hwnd, lparam) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != pid || !IsWindowVisible(hwnd)) { return true; }
            if (!string.IsNullOrEmpty(titleLike)) {
                var text = new StringBuilder(512);
                GetWindowText(hwnd, text, text.Capacity);
                if (text.ToString().IndexOf(titleLike, StringComparison.OrdinalIgnoreCase) < 0) {
                    return true;
                }
            }
            found.Add(hwnd);
            return true;
        }, IntPtr.Zero);
        return found.ToArray();
    }

    public static bool Close(IntPtr hwnd) {
        // WM_CLOSE = 0x0010：与用户点标题栏的 × 走同一条 `CloseRequested` 路径
        SendMessage(hwnd, 0x0010, IntPtr.Zero, IntPtr.Zero);
        return true;
    }
}
'@
}

$WM_CLOSE = 0x0010
$deadline = (Get-Date).AddSeconds($WaitSeconds)
$targets = @()
while ((Get-Date) -lt $deadline) {
    foreach ($proc in (Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)) {
        $windows = [WinClose]::FindWindows([uint32]$proc.Id, $TitleLike)
        if ($windows.Length -gt 0) {
            $targets = $windows
            break
        }
    }
    if ($targets.Length -gt 0) { break }
    Start-Sleep -Milliseconds 400
}

if ($targets.Length -eq 0) {
    $hint = if ($TitleLike) { "（标题包含 '$TitleLike'）" } else { "" }
    Write-Host "没有找到进程 '$ProcessName' 的可见窗口$hint" -ForegroundColor Red
    exit 1
}

$count = if ($All) { $targets.Length } else { 1 }
for ($i = 0; $i -lt $count; $i++) {
    [void][WinClose]::SendMessage($targets[$i], $WM_CLOSE, [IntPtr]::Zero, [IntPtr]::Zero)
    Write-Host "已向窗口发送 WM_CLOSE: $($targets[$i])"
}

exit 0
