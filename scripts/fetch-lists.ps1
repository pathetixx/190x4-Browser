<#
  Скачивает фильтр-списки в src-tauri/lists.

  Списки не хранятся в репозитории: это чужие данные, которые обновляются
  ежедневно и раздуют историю. Скрипт зовётся перед сборкой бандла и в CI.
#>

$ErrorActionPreference = "Stop"

$target = Join-Path $PSScriptRoot "..\src-tauri\lists"
New-Item -ItemType Directory -Force -Path $target | Out-Null

$lists = @{
    "easylist.txt"    = "https://easylist.to/easylist/easylist.txt"
    "easyprivacy.txt" = "https://easylist.to/easylist/easyprivacy.txt"
    # Без EasyList внутри: он и так идёт отдельным списком, а вдвоём его
    # правила разбирались и хранились дважды.
    "ruadlist.txt"    = "https://easylist-downloads.adblockplus.org/advblock+cssfixes.txt"
}

foreach ($name in $lists.Keys) {
    $path = Join-Path $target $name
    Write-Host "→ $name"
    Invoke-WebRequest -Uri $lists[$name] -OutFile $path -UseBasicParsing
    $size = [math]::Round((Get-Item $path).Length / 1KB)
    Write-Host "  $size КБ"
}
