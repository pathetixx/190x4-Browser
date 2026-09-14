# Reference measurement: Edge on the same 20 sites as browser190x4-spike memory.
# Without it "3.4 GB" is a number without meaning.
#
# ASCII only: PowerShell 5.1 reads a BOM-less .ps1 as ANSI and chokes on Cyrillic.
# Uses a throwaway user-data-dir so the user's own Edge profile is untouched.

param([int]$Settle = 20)

$ErrorActionPreference = "Stop"

$profileDir = "E:\test\190x4-browser\spike-out\edge-profile"
$outFile    = "E:\test\190x4-browser\spike-out\edge-baseline.json"

$urls = @(
  "https://www.kinopoisk.ru/",
  "https://dzen.ru/",
  "https://vk.com/",
  "https://www.youtube.com/",
  "https://habr.com/ru/feed/",
  "https://lenta.ru/",
  "https://www.ozon.ru/",
  "https://www.wildberries.ru/",
  "https://www.avito.ru/",
  "https://rutube.ru/",
  "https://pikabu.ru/",
  "https://github.com/explore",
  "https://www.reddit.com/",
  "https://mail.ru/",
  "https://www.gismeteo.ru/",
  "https://tass.ru/",
  "https://sports.ru/",
  "https://www.twitch.tv/",
  "https://ya.ru/",
  "https://t.me/s/durov"
)

$edge = "${env:ProgramFiles(x86)}\Microsoft\Edge\Application\msedge.exe"
if (-not (Test-Path $edge)) { $edge = "$env:ProgramFiles\Microsoft\Edge\Application\msedge.exe" }

$common = @("--user-data-dir=$profileDir", "--no-first-run", "--no-default-browser-check")

Start-Process $edge -ArgumentList ($common + @("--new-window", $urls[0]))
Start-Sleep -Seconds 8

foreach ($u in $urls[1..($urls.Count - 1)]) {
  Start-Process $edge -ArgumentList ($common + @($u))
  # Same cadence as the spike: 3 s between tabs, so the network is not the bottleneck.
  Start-Sleep -Seconds 3
}

Start-Sleep -Seconds $Settle

$procs = Get-CimInstance Win32_Process -Filter "Name='msedge.exe'" |
  Where-Object { $_.CommandLine -like "*$profileDir*" }

$private = 0
$working = 0
$count = 0
foreach ($p in $procs) {
  $ps = Get-Process -Id $p.ProcessId -ErrorAction SilentlyContinue
  if ($ps) {
    $private += $ps.PrivateMemorySize64
    $working += $ps.WorkingSet64
    $count++
  }
}

[pscustomobject]@{
  browser        = "Edge"
  tabs           = $urls.Count
  processes      = $count
  private_mb     = [math]::Round($private / 1MB, 1)
  working_set_mb = [math]::Round($working / 1MB, 1)
} | ConvertTo-Json | Set-Content -Path $outFile -Encoding ASCII

foreach ($p in $procs) { Stop-Process -Id $p.ProcessId -Force -ErrorAction SilentlyContinue }
