$deadline = (Get-Date).AddSeconds(60)
while ((Get-ScheduledTask -TaskName "190x4ProbeCapture").State -eq "Running" -and (Get-Date) -lt $deadline) { Start-Sleep -Milliseconds 300 }
Unregister-ScheduledTask -TaskName "190x4ProbeCapture" -Confirm:$false
Get-Content "E:\test\probe\capture.txt"
