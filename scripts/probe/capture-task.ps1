param([int]$Frames = 70)
$out = "E:\test\probe\capture.txt"
Remove-Item $out, "E:\test\probe\ready.txt" -ErrorAction SilentlyContinue
$arg = '-NoProfile -ExecutionPolicy Bypass -WindowStyle Hidden -Command "& ''E:\test\probe\capture.ps1'' -Frames ' + $Frames + ' *> ''' + $out + '''"'
$a = New-ScheduledTaskAction -Execute "powershell.exe" -Argument $arg
$p = New-ScheduledTaskPrincipal -UserId "Administrator" -LogonType Interactive -RunLevel Highest
Register-ScheduledTask -TaskName "190x4ProbeCapture" -Action $a -Principal $p -Force | Out-Null
Start-ScheduledTask -TaskName "190x4ProbeCapture"
"started"
