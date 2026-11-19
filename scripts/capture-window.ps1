param(
  [Parameter(Mandatory = $true)][string]$OutDir,
  [string]$ProcessName = "coconut-app",
  [int]$WaitForWindowSeconds = 25,
  [int]$SettleMilliseconds = 900,
  [int]$Count = 3,
  [int]$IntervalMs = 190
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public class Win32Capture {
  [StructLayout(LayoutKind.Sequential)]
  public struct RECT { public int Left; public int Top; public int Right; public int Bottom; }
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);
  [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr hWnd, IntPtr hdcBlt, uint nFlags);
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("shcore.dll")] public static extern int SetProcessDpiAwareness(int value);
}
"@

try { [void][Win32Capture]::SetProcessDpiAwareness(2) } catch { }
try { [void][Win32Capture]::SetProcessDPIAware() } catch { }

$deadline = (Get-Date).AddSeconds($WaitForWindowSeconds)
$proc = $null
while ((Get-Date) -lt $deadline) {
  $proc = Get-Process -Name $ProcessName -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
  if ($proc) { break }
  Start-Sleep -Milliseconds 250
}
if (-not $proc) { throw "no main window for $ProcessName within $WaitForWindowSeconds s" }

$hwnd = $proc.MainWindowHandle
Start-Sleep -Milliseconds $SettleMilliseconds

$rect = New-Object Win32Capture+RECT
[void][Win32Capture]::GetWindowRect($hwnd, [ref]$rect)
$w = $rect.Right - $rect.Left
$h = $rect.Bottom - $rect.Top
if ($w -le 0 -or $h -le 0) { throw "bad window rect: ${w}x${h}" }

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir -Force | Out-Null }
Get-ChildItem $OutDir -Filter *.png -ErrorAction SilentlyContinue | Remove-Item -Force

$bmp = New-Object System.Drawing.Bitmap $w, $h
for ($i = 1; $i -le $Count; $i++) {
  $g = [System.Drawing.Graphics]::FromImage($bmp)
  $hdc = $g.GetHdc()
  $ok = [Win32Capture]::PrintWindow($hwnd, $hdc, 2)
  $g.ReleaseHdc($hdc)
  $g.Dispose()
  if (-not $ok) { throw "PrintWindow failed" }
  $file = Join-Path $OutDir ("frame-{0}.png" -f $i)
  $bmp.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
  if ($i -lt $Count) { Start-Sleep -Milliseconds $IntervalMs }
}
$bmp.Dispose()
Write-Output ("captured " + $Count + " frames " + $w + "x" + $h + " -> " + $OutDir)
