# 把外壳的某个窗口切到前台（自动前进的"焦点失效就关掉"要真的让窗口有焦点才能测）。
#
# 用法：
#   pwsh -File tauri/tools/focus-window.ps1 -ProcessName anki-tauri -TitleLike "Anki"
#
# 退出码：0 = 已切到前台；1 = 没找到窗口。

[CmdletBinding()]
param(
    [string]$ProcessName = "anki-tauri",
    [string]$TitleLike = "",
    [int]$WaitSeconds = 10
)

$ErrorActionPreference = "Stop"

if (-not ("WinFocus" -as [type])) {
    Add-Type @'
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;

public class WinFocus {
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, StringBuilder text, int max);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);

    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lparam);

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
}
'@
}

$deadline = (Get-Date).AddSeconds($WaitSeconds)
$target = [IntPtr]::Zero
while ((Get-Date) -lt $deadline) {
    foreach ($proc in (Get-Process -Name $ProcessName -ErrorAction SilentlyContinue)) {
        $windows = [WinFocus]::FindWindows([uint32]$proc.Id, $TitleLike)
        if ($windows.Length -gt 0) { $target = $windows[0]; break }
    }
    if ($target -ne [IntPtr]::Zero) { break }
    Start-Sleep -Milliseconds 400
}

if ($target -eq [IntPtr]::Zero) {
    $hint = if ($TitleLike) { "（标题包含 '$TitleLike'）" } else { "" }
    Write-Host "没有找到进程 '$ProcessName' 的可见窗口$hint" -ForegroundColor Red
    exit 1
}

# SW_RESTORE = 9：最小化时先恢复，再拿到前台
[void][WinFocus]::ShowWindow($target, 9)
[void][WinFocus]::SetForegroundWindow($target)
Write-Host "已把窗口切到前台: $target"
exit 0
