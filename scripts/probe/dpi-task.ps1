param([int]$Percent = 0)
$out = "E:\test\probe\dpi.txt"
Remove-Item $out -ErrorAction SilentlyContinue
$arg = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "& ''E:\test\probe\dpi.ps1'' -Percent ' + $Percent + ' *> ''' + $out + '''"'
$a = New-ScheduledTaskAction -Execute "powershell.exe" -Argument $arg
$p = New-ScheduledTaskPrincipal -UserId "Administrator" -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName "190x4ProbeDpi" -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "190x4ProbeDpi"
Start-Sleep -Milliseconds 800
$deadline = (Get-Date).AddSeconds(20)
while ((Get-ScheduledTask -TaskName "190x4ProbeDpi").State -eq "Running" -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 200 }
Unregister-ScheduledTask -TaskName "190x4ProbeDpi" -Confirm:$false
if (Test-Path $out) { Get-Content $out } else { "no dpi output" }
