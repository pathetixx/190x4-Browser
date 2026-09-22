param([int]$Vk = 0, [int]$Mod = 0, [int]$X = -1, [int]$Y = -1, [int]$Right = 0)
$out = "E:\test\probe\input.txt"
Remove-Item $out -ErrorAction SilentlyContinue
$inner = "& 'E:\test\probe\key.ps1' -Vk $Vk -Mod $Mod -X $X -Y $Y -Right $Right"
$arg = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "' + $inner + ' *> ''' + $out + '''"'
$a = New-ScheduledTaskAction -Execute "powershell.exe" -Argument $arg
$p = New-ScheduledTaskPrincipal -UserId "Administrator" -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName "190x4ProbeInput" -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "190x4ProbeInput"
Start-Sleep -Milliseconds 800
$deadline = (Get-Date).AddSeconds(20)
while ((Get-ScheduledTask -TaskName "190x4ProbeInput").State -eq "Running" -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
Unregister-ScheduledTask -TaskName "190x4ProbeInput" -Confirm:$false
if (Test-Path $out) { Get-Content $out } else { "no input output" }
