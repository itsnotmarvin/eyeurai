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

$markerPath = Join-Path $installDir "native-bridge-ready.txt"
$app = Start-Process `
  -FilePath $executable.FullName `
  -ArgumentList "--startup-smoke-marker=$markerPath" `
  -PassThru

$deadline = (Get-Date).AddSeconds(20)
while (-not (Test-Path $markerPath) -and (Get-Date) -lt $deadline) {
  if ($app.HasExited) {
    throw "EyeUrAI exited before its native bridge became ready with code $($app.ExitCode)"
  }
  Start-Sleep -Milliseconds 250
}

if (-not (Test-Path $markerPath)) {
  Stop-Process -Id $app.Id -Force
  throw "EyeUrAI started, but its packaged frontend never reached the native command bridge"
}

$marker = (Get-Content -Raw $markerPath).Trim()
if ($marker -ne "native-bridge-ready") {
  Stop-Process -Id $app.Id -Force
  throw "EyeUrAI wrote an invalid native bridge smoke marker: $marker"
}

Stop-Process -Id $app.Id -Force
Write-Host "EyeUrAI installed and its packaged frontend reached the native bridge."
