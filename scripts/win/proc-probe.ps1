# Clean check: does one Environment really mean one browser process?
# Kill leftovers first - a stale instance from a previous run doubles every count.
Get-Process -Name 190x4-browser -ErrorAction SilentlyContinue | Stop-Process -Force
Get-Process -Name msedgewebview2 -ErrorAction SilentlyContinue |
  Where-Object { $_.Path -like "*Edge*" } | Out-Null
Start-Sleep -Seconds 3

$before = @(Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'").Count
"webview2 processes before start: $before"

Start-Process "E:\test\190x4-browser\target\release\190x4-browser.exe"
Start-Sleep -Seconds 16

$app = Get-Process -Name 190x4-browser -ErrorAction SilentlyContinue | Select-Object -First 1
"app pid: $($app.Id)"

$all = Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'"
foreach ($p in $all) {
  $t = "browser"
  if ($p.CommandLine -match "--type=([a-z-]+)") { $t = $Matches[1] }
  "{0,-18} pid={1,-6} parent={2}" -f $t, $p.ProcessId, $p.ParentProcessId
}

Get-Process -Name 190x4-browser -ErrorAction SilentlyContinue | Stop-Process -Force
