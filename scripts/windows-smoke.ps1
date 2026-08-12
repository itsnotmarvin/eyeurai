param(
  [Parameter(Mandatory = $true)]
  [string]$Installer
)

$ErrorActionPreference = "Stop"

$installerPath = (Resolve-Path $Installer).Path
$runId = if ($env:GITHUB_RUN_ID) { $env:GITHUB_RUN_ID } else { $PID }
$runAttempt = if ($env:GITHUB_RUN_ATTEMPT) { $env:GITHUB_RUN_ATTEMPT } else { "local" }
$installDir = Join-Path $env:RUNNER_TEMP "EyeUrAI-smoke-$runId-$runAttempt"

$install = Start-Process `
  -FilePath $installerPath `
  -ArgumentList "/S", "/D=$installDir" `
  -Wait `
  -PassThru
if ($install.ExitCode -ne 0) {
  throw "EyeUrAI installer exited with code $($install.ExitCode)"
}

$executable = Get-ChildItem -Path $installDir -Filter "eyeurai.exe" -Recurse |
  Select-Object -First 1
if (-not $executable) {
  throw "EyeUrAI executable was not installed under $installDir"
}

$app = Start-Process -FilePath $executable.FullName -PassThru
Start-Sleep -Seconds 10
if ($app.HasExited) {
  throw "EyeUrAI exited during its Windows startup smoke test with code $($app.ExitCode)"
}

Stop-Process -Id $app.Id -Force
Write-Host "EyeUrAI installed and remained running for the Windows startup smoke test."
