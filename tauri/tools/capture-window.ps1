# 截取指定进程的主窗口，用于 Tauri 外壳的视觉验证。
#
# 两种抓取方式：
#   PrintWindow(PW_RENDERFULLCONTENT) —— 不受遮挡影响，但对 WebView2 这类
#     GPU 合成的窗口可能抓到错位/缺失的内容。
#   Screen（CopyFromScreen）—— 所见即所得，要求窗口可见且未被遮挡。
# 需要对比判断是渲染问题还是抓取伪影时，两种都跑一遍。
#
# 用法：
#   pwsh -File tauri/tools/capture-window.ps1 -OutFile out/shot.png
#   pwsh -File tauri/tools/capture-window.ps1 -OutFile out/shot.png -Method Screen
#   pwsh -File tauri/tools/capture-window.ps1 -OutFile out/shot.png -TitleLike "deck options"
#
# 多窗口模型下（见 `src/windows.rs`）同一进程会有多个顶层窗口，`Get-Process` 的
# `MainWindowHandle` 只给出其中一个，因此这里枚举该进程的全部可见顶层窗口：
# 不给 `-TitleLike` 时取 z 序最上面那个，给了就按标题子串筛选。

[CmdletBinding()]
param(
    [string]$ProcessName = "anki-tauri",
    [Parameter(Mandatory = $true)][string]$OutFile,
    [int]$WaitSeconds = 20,
    [ValidateSet("Auto", "PrintWindow", "Screen")][string]$Method = "Auto",
    [string]$TitleLike = ""
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName System.Drawing

if (-not ("WinCap" -as [type])) {
    Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class WinCap {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }

    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hwnd, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hwnd, out RECT rect);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool ShowWindow(IntPtr hwnd, int cmd);
    [DllImport("user32.dll")] public static extern bool IsIconic(IntPtr hwnd);
    [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hwnd);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lparam);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hwnd, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowText(IntPtr hwnd, System.Text.StringBuilder text, int max);

    public delegate bool EnumWindowsProc(IntPtr hwnd, IntPtr lparam);

    /// 枚举某个进程的全部可见顶层窗口（按 z 序，最上面的在前）。
    public static IntPtr[] FindWindows(uint pid, string titleLike) {
        var found = new System.Collections.Generic.List<IntPtr>();
        EnumWindows((hwnd, lparam) => {
            uint owner;
            GetWindowThreadProcessId(hwnd, out owner);
            if (owner != pid || !IsWindowVisible(hwnd)) { return true; }
            if (!string.IsNullOrEmpty(titleLike)) {
                var text = new System.Text.StringBuilder(512);
                GetWindowText(hwnd, text, text.Capacity);
                if (text.ToString().IndexOf(titleLike, System.StringComparison.OrdinalIgnoreCase) < 0) {
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

# PW_RENDERFULLCONTENT：抓取窗口内容而非仅重绘
$PW_RENDERFULLCONTENT = 2

# 必须在任何 GetWindowRect / 截图 API 之前声明 DPI 感知。
# 否则在缩放 >100% 的屏幕上：GetWindowRect 返回被系统虚拟化的偏小坐标，而 GDI
# 按物理像素拷贝同样大小的区域 —— 得到"窗口只截到一部分、右侧按钮消失"的假象
# （实测 150% 缩放下只截到窗口的 2/3，会误判成布局缺陷）。
$dpiAware = $false
try {
    $dpiAware = [WinCap]::SetProcessDpiAwareness(2) -eq 0
} catch { }
if (-not $dpiAware) {
    try { $dpiAware = [WinCap]::SetProcessDPIAware() } catch { }
}
if (-not $dpiAware) {
    Write-Warning "无法声明 DPI 感知，截取坐标可能被系统虚拟化"
}

function Get-TargetWindow {
    param([string]$Name, [int]$TimeoutSeconds, [string]$TitleLike)
    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    while ((Get-Date) -lt $deadline) {
        foreach ($proc in (Get-Process -Name $Name -ErrorAction SilentlyContinue)) {
            $windows = [WinCap]::FindWindows([uint32]$proc.Id, $TitleLike)
            if ($windows.Length -gt 0) {
                return @{ Process = $proc; Hwnd = $windows[0] }
            }
        }
        Start-Sleep -Milliseconds 500
    }
    return $null
}

function Test-Blank {
    param([System.Drawing.Bitmap]$Bitmap)
    # 采样网格，如果只有一个颜色，视为未渲染（WebView2 常见黑屏）
    $first = $null
    for ($x = 0; $x -lt $Bitmap.Width; $x += [Math]::Max(1, [int]($Bitmap.Width / 8))) {
        for ($y = 0; $y -lt $Bitmap.Height; $y += [Math]::Max(1, [int]($Bitmap.Height / 8))) {
            $c = $Bitmap.GetPixel($x, $y)
            if ($null -eq $first) { $first = $c } elseif ($c -ne $first) { return $false }
        }
    }
    return $true
}

function Get-WindowBitmap {
    param([IntPtr]$Hwnd, [int]$Width, [int]$Height, [WinCap+RECT]$Rect, [string]$Mode)
    if ($Mode -eq "Screen") {
        # 屏幕抓取要求窗口在前台且未最小化
        if ([WinCap]::IsIconic($Hwnd)) { [WinCap]::ShowWindow($Hwnd, 9) | Out-Null }
        [WinCap]::SetForegroundWindow($Hwnd) | Out-Null
        Start-Sleep -Milliseconds 900
        $bmp = New-Object System.Drawing.Bitmap $Width, $Height
        $gfx = [System.Drawing.Graphics]::FromImage($bmp)
        $gfx.CopyFromScreen($Rect.Left, $Rect.Top, 0, 0, (New-Object System.Drawing.Size $Width, $Height))
        $gfx.Dispose()
        return @{ Bitmap = $bmp; Printed = $null }
    }

    $bmp = New-Object System.Drawing.Bitmap $Width, $Height
    $gfx = [System.Drawing.Graphics]::FromImage($bmp)
    $hdc = $gfx.GetHdc()
    $printed = $false
    try {
        $printed = [WinCap]::PrintWindow($Hwnd, $hdc, $PW_RENDERFULLCONTENT)
    } finally {
        $gfx.ReleaseHdc($hdc)
        $gfx.Dispose()
    }
    return @{ Bitmap = $bmp; Printed = $printed }
}

$target = Get-TargetWindow -Name $ProcessName -TimeoutSeconds $WaitSeconds -TitleLike $TitleLike
if (-not $target) {
    $hint = if ($TitleLike) { "（标题包含 '$TitleLike'）" } else { "" }
    throw "在 $WaitSeconds 秒内没有找到进程 '$ProcessName' 的可见窗口$hint"
}
$proc = $target.Process
$hwnd = $target.Hwnd

$rect = New-Object WinCap+RECT
if (-not [WinCap]::GetWindowRect($hwnd, [ref]$rect)) {
    throw "GetWindowRect 失败（hwnd=$hwnd）"
}

$width = $rect.Right - $rect.Left
$height = $rect.Bottom - $rect.Top
if ($width -le 0 -or $height -le 0) {
    throw "窗口尺寸无效: ${width}x${height}"
}

$usedMethod = $Method
if ($Method -eq "Screen") {
    $result = Get-WindowBitmap -Hwnd $hwnd -Width $width -Height $height -Rect $rect -Mode "Screen"
} else {
    $result = Get-WindowBitmap -Hwnd $hwnd -Width $width -Height $height -Rect $rect -Mode "PrintWindow"
    $bitmap = $result.Bitmap
    $blank = Test-Blank -Bitmap $bitmap
    if ($result.Printed -and -not $blank) {
        $usedMethod = "PrintWindow"
    } elseif ($Method -eq "PrintWindow") {
        $bitmap.Dispose()
        throw "PrintWindow 未能取得有效内容（printed=$($result.Printed) blank=$blank）"
    } else {
        # Auto：PrintWindow 不可靠时回退到屏幕抓取
        $bitmap.Dispose()
        Write-Warning "PrintWindow 结果不可靠（printed=$($result.Printed) blank=$blank），回退到屏幕抓取"
        $result = Get-WindowBitmap -Hwnd $hwnd -Width $width -Height $height -Rect $rect -Mode "Screen"
        $usedMethod = "Screen(fallback)"
    }
}

$bitmap = $result.Bitmap
$dir = Split-Path -Parent $OutFile
if ($dir -and -not (Test-Path $dir)) { New-Item -ItemType Directory -Force -Path $dir | Out-Null }
$bitmap.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bitmap.Dispose()

# 输出机器可读的一行，便于脚本断言
"captured method=$usedMethod size=${width}x${height} pid=$($proc.Id) file=$OutFile"
