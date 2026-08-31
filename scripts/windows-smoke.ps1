param(
  [Parameter(Mandatory = $true)]
  [string]$Installer
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 2.0

if (-not $env:APPDATA) {
  throw "APPDATA is unavailable; the Windows updater visibility marker cannot be tested safely."
}

$updateMarkerDirectory = Join-Path $env:APPDATA "com.eyeurai.desktop"
$updateMarkerPath = Join-Path $updateMarkerDirectory "update-relaunch-visible-v1"
$updateMarkerTempPath = "$updateMarkerPath.tmp"

# The installer hook replaces this one-use marker during an update. Never run
# against a real marker (or its in-progress temporary file) that was present
# before the smoke test; launching any app could consume it as well.
if ((Test-Path -LiteralPath $updateMarkerPath) -or
    (Test-Path -LiteralPath $updateMarkerTempPath)) {
  throw "Refusing to run: an EyeUrAI update visibility marker already exists at $updateMarkerPath"
}

$preExistingApps = @(Get-Process -Name "eyeurai" -ErrorAction SilentlyContinue)
if ($preExistingApps.Count -gt 0) {
  $preExistingProcessIds = ($preExistingApps | ForEach-Object { $_.Id }) -join ", "
  throw "Refusing to run while a pre-existing EyeUrAI process is active (PID(s): $preExistingProcessIds)."
}

$installerPath = (Resolve-Path -LiteralPath $Installer).Path
$runId = if ($env:GITHUB_RUN_ID) { $env:GITHUB_RUN_ID } else { $PID }
$runAttempt = if ($env:GITHUB_RUN_ATTEMPT) { $env:GITHUB_RUN_ATTEMPT } else { "local" }
$tempRoot = if ($env:RUNNER_TEMP) { $env:RUNNER_TEMP } else { [System.IO.Path]::GetTempPath() }
$installDir = Join-Path $tempRoot "EyeUrAI-smoke-$runId-$runAttempt-$([Guid]::NewGuid().ToString('N'))"
$markerDirectory = Join-Path $tempRoot "EyeUrAI-smoke-markers-$runId-$runAttempt-$([Guid]::NewGuid().ToString('N'))"
[void][System.IO.Directory]::CreateDirectory($markerDirectory)

if (-not ("EyeUrAI.WindowsSmoke.NativeMethods" -as [type])) {
  Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
using System.Text;

namespace EyeUrAI.WindowsSmoke
{
    public static class NativeMethods
    {
        private const string EyeUrAITitle = "EyeUrAI";
        private const string EyeUrAIWindowClass = "Tauri Window";
        private const int GWL_EXSTYLE = -20;
        private const int WS_EX_TOOLWINDOW = 0x00000080;

        [return: MarshalAs(UnmanagedType.Bool)]
        private delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);

        [StructLayout(LayoutKind.Sequential)]
        private struct RECT
        {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool EnumWindows(EnumWindowsProc callback, IntPtr lParam);

        [DllImport("user32.dll")]
        private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetClassNameW(IntPtr hWnd, StringBuilder className, int maxCount);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetWindowTextLengthW(IntPtr hWnd);

        [DllImport("user32.dll", CharSet = CharSet.Unicode)]
        private static extern int GetWindowTextW(IntPtr hWnd, StringBuilder text, int maxCount);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool GetWindowRect(IntPtr hWnd, out RECT rect);

        [DllImport("user32.dll", EntryPoint = "GetWindowLongW")]
        private static extern int GetWindowLong(IntPtr hWnd, int index);

        [DllImport("user32.dll", SetLastError = true)]
        [return: MarshalAs(UnmanagedType.Bool)]
        private static extern bool IsWindowVisible(IntPtr hWnd);

        private static IntPtr FindEyeUrAIMainWindow(int processId)
        {
            // Tao owns a zero-sized, technically visible event-target HWND.
            // Match the real configured Tauri window instead of trusting
            // Process.MainWindowHandle, which can select Tao's helper.
            var found = IntPtr.Zero;
            EnumWindows((hWnd, _) =>
            {
                GetWindowThreadProcessId(hWnd, out var ownerProcessId);
                if (ownerProcessId != (uint)processId)
                {
                    return true;
                }

                var className = new StringBuilder(256);
                if (GetClassNameW(hWnd, className, className.Capacity) != EyeUrAIWindowClass.Length ||
                    !string.Equals(className.ToString(), EyeUrAIWindowClass, StringComparison.Ordinal))
                {
                    return true;
                }

                var titleLength = GetWindowTextLengthW(hWnd);
                if (titleLength != EyeUrAITitle.Length)
                {
                    return true;
                }
                var title = new StringBuilder(titleLength + 1);
                if (GetWindowTextW(hWnd, title, title.Capacity) != titleLength ||
                    !string.Equals(title.ToString(), EyeUrAITitle, StringComparison.Ordinal))
                {
                    return true;
                }

                if (!GetWindowRect(hWnd, out var bounds) ||
                    bounds.Right <= bounds.Left ||
                    bounds.Bottom <= bounds.Top ||
                    (GetWindowLong(hWnd, GWL_EXSTYLE) & WS_EX_TOOLWINDOW) != 0)
                {
                    return true;
                }

                found = hWnd;
                return false;
            }, IntPtr.Zero);
            return found;
        }

        public static bool HasEyeUrAIMainWindow(int processId)
        {
            return FindEyeUrAIMainWindow(processId) != IntPtr.Zero;
        }

        public static bool IsEyeUrAIMainWindowVisible(int processId)
        {
            var window = FindEyeUrAIMainWindow(processId);
            return window != IntPtr.Zero && IsWindowVisible(window);
        }
    }
}
"@
}

function ConvertTo-WindowsCommandLineArgument {
  param(
    [Parameter(Mandatory = $true)]
    [AllowEmptyString()]
    [string]$Argument
  )

  if ($Argument.Length -gt 0 -and $Argument -notmatch '[\s"]') {
    return $Argument
  }

  # Follow CommandLineToArgvW/CRT escaping: double backslashes before a quote,
  # and double trailing backslashes before the closing quote.
  $escaped = [regex]::Replace($Argument, '(\\*)"', '$1$1\"')
  $escaped = [regex]::Replace($escaped, '(\\+)$', '$1$1')
  return '"' + $escaped + '"'
}

function Start-SmokeProcess {
  param(
    [Parameter(Mandatory = $true)]
    [string]$FilePath,
    [Parameter(Mandatory = $true)]
    [AllowEmptyCollection()]
    [string[]]$Arguments
  )

  $argumentLine = ($Arguments | ForEach-Object {
      ConvertTo-WindowsCommandLineArgument -Argument $_
    }) -join ' '

  return Start-Process `
    -FilePath $FilePath `
    -ArgumentList $argumentLine `
    -PassThru
}

function New-StartupMarkerLease {
  param(
    [Parameter(Mandatory = $true)]
    [string]$Name
  )

  $markerPath = Join-Path $markerDirectory "$Name-$([Guid]::NewGuid().ToString('N')).txt"
  [System.IO.File]::WriteAllText($markerPath, "")
  return $markerPath
}

function Assert-WindowsGuiSubsystem {
  param(
    [Parameter(Mandatory = $true)]
    [string]$ExecutablePath
  )

  $stream = [System.IO.File]::OpenRead($ExecutablePath)
  $reader = [System.IO.BinaryReader]::new($stream)
  try {
    if ($stream.Length -lt 64) {
      throw "EyeUrAI's executable is too small to contain a valid PE header."
    }

    $stream.Position = 0x3c
    $peOffset = $reader.ReadInt32()
    $subsystemEnd = [int64]$peOffset + 24 + 70
    if ($peOffset -lt 0 -or $subsystemEnd -gt $stream.Length) {
      throw "EyeUrAI's executable contains an invalid PE header offset."
    }

    $stream.Position = $peOffset
    if ($reader.ReadUInt32() -ne 0x00004550) {
      throw "EyeUrAI's executable is missing the PE signature."
    }

    $stream.Position = $peOffset + 24
    $optionalHeaderMagic = $reader.ReadUInt16()
    if ($optionalHeaderMagic -ne 0x010b -and $optionalHeaderMagic -ne 0x020b) {
      throw "EyeUrAI's executable has an unsupported PE optional-header format."
    }

    # The Subsystem field is 68 bytes into both PE32 and PE32+ optional headers.
    $stream.Position = $peOffset + 24 + 68
    $subsystem = $reader.ReadUInt16()
    if ($subsystem -ne 2) {
      throw "EyeUrAI's packaged executable uses PE subsystem $subsystem; expected Windows GUI (2)."
    }
  } finally {
    $reader.Dispose()
  }
}

function Read-StartupMarker {
  param(
    [Parameter(Mandatory = $true)]
    [string]$MarkerPath
  )

  try {
    $contents = [System.IO.File]::ReadAllText($MarkerPath).Trim()
  } catch [System.IO.IOException] {
    # The app truncates and rewrites this tiny file as its state advances.
    return $null
  }

  if (-not $contents) {
    return $null
  }

  $match = [regex]::Match(
    $contents,
    '^native-bridge-(started|ready):([1-9][0-9]*)$'
  )
  if (-not $match.Success) {
    return [PSCustomObject]@{
      IsValid = $false
      Raw = $contents
      Phase = $null
      ProcessId = 0
    }
  }

  return [PSCustomObject]@{
    IsValid = $true
    Raw = $contents
    Phase = $match.Groups[1].Value
    ProcessId = [int64]$match.Groups[2].Value
  }
}

function Wait-NativeBridgeReady {
  param(
    [Parameter(Mandatory = $true)]
    [string]$MarkerPath,
    [int64]$ExpectedProcessId = 0,
    [int]$TimeoutSeconds = 30
  )

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  $observedProcessId = [int64]0
  $lastInvalidMarker = $null

  while ((Get-Date) -lt $deadline) {
    if ($ExpectedProcessId -gt 0 -and
        -not (Get-Process -Id $ExpectedProcessId -ErrorAction SilentlyContinue)) {
      throw "EyeUrAI process $ExpectedProcessId exited before its native bridge became ready."
    }

    $marker = Read-StartupMarker -MarkerPath $MarkerPath
    if ($null -ne $marker) {
      if (-not $marker.IsValid) {
        # Do not fail on a transient partial read while the marker is being
        # rewritten. If it never becomes valid, report its last contents.
        $lastInvalidMarker = $marker.Raw
      } else {
        $lastInvalidMarker = $null
        $markerProcessId = [int64]$marker.ProcessId
        if ($observedProcessId -gt 0 -and $markerProcessId -ne $observedProcessId) {
          throw "EyeUrAI changed native bridge process IDs from $observedProcessId to $markerProcessId during startup."
        }
        if ($ExpectedProcessId -gt 0 -and $markerProcessId -ne $ExpectedProcessId) {
          throw "EyeUrAI reported native bridge process $markerProcessId, expected $ExpectedProcessId."
        }

        $observedProcessId = $markerProcessId
        if (-not (Get-Process -Id $observedProcessId -ErrorAction SilentlyContinue)) {
          throw "EyeUrAI process $observedProcessId exited before its native bridge became ready."
        }
        if ($marker.Phase -eq "ready") {
          return $observedProcessId
        }
      }
    }

    if ($observedProcessId -gt 0 -and
        -not (Get-Process -Id $observedProcessId -ErrorAction SilentlyContinue)) {
      throw "EyeUrAI process $observedProcessId exited before its native bridge became ready."
    }

    Start-Sleep -Milliseconds 250
  }

  if ($lastInvalidMarker) {
    throw "EyeUrAI wrote an invalid native bridge smoke marker: $lastInvalidMarker"
  }
  throw "EyeUrAI started, but its packaged frontend never reached the native command bridge."
}

function Test-MainWindowVisible {
  param(
    [Parameter(Mandatory = $true)]
    [int64]$ProcessId
  )

  return [EyeUrAI.WindowsSmoke.NativeMethods]::IsEyeUrAIMainWindowVisible(
    [int]$ProcessId
  )
}

function Test-MainWindowExists {
  param(
    [Parameter(Mandatory = $true)]
    [int64]$ProcessId
  )

  return [EyeUrAI.WindowsSmoke.NativeMethods]::HasEyeUrAIMainWindow(
    [int]$ProcessId
  )
}

function Wait-MainWindowVisible {
  param(
    [Parameter(Mandatory = $true)]
    [int64]$ProcessId,
    [int]$TimeoutSeconds = 10
  )

  $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
      throw "EyeUrAI process $ProcessId exited before its main window became visible."
    }
    if (Test-MainWindowVisible -ProcessId $ProcessId) {
      return
    }
    Start-Sleep -Milliseconds 200
  }

  throw "EyeUrAI process $ProcessId reached its native bridge, but its main window was not visible."
}

function Assert-MainWindowRemainsHidden {
  param(
    [Parameter(Mandatory = $true)]
    [int64]$ProcessId,
    [int]$ObservationSeconds = 3
  )

  $deadline = (Get-Date).AddSeconds($ObservationSeconds)
  while ((Get-Date) -lt $deadline) {
    if (-not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
      throw "Hidden EyeUrAI process $ProcessId exited during the visibility check."
    }
    if (-not (Test-MainWindowExists -ProcessId $ProcessId)) {
      throw "EyeUrAI process $ProcessId did not create its expected main window."
    }
    if (Test-MainWindowVisible -ProcessId $ProcessId) {
      throw "EyeUrAI process $ProcessId showed its main window despite a plain --hidden launch."
    }
    Start-Sleep -Milliseconds 200
  }
}

function Stop-SmokeProcess {
  param(
    [int64]$ProcessId
  )

  if ($ProcessId -le 0) {
    return
  }
  $process = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
  if ($null -ne $process) {
    Stop-Process -Id $ProcessId -Force -ErrorAction SilentlyContinue
    try {
      [void]$process.WaitForExit(5000)
    } catch {
      # The process already exited or its handle became unavailable.
    }
  }
}

function Stop-ProcessesForMarker {
  param(
    [string]$ExecutablePath,
    [string]$MarkerPath
  )

  if (-not $ExecutablePath -or -not $MarkerPath) {
    return
  }

  # If startup failed before a valid PID could be read, the unique marker
  # argument still lets cleanup find the exact smoke instance without touching
  # a pre-existing user process.
  $markerArgument = "--startup-smoke-marker=$MarkerPath"
  $marker = Read-StartupMarker -MarkerPath $MarkerPath
  if ($null -ne $marker -and $marker.IsValid) {
    Stop-SmokeProcess -ProcessId ([int64]$marker.ProcessId)
  }

  try {
    $processes = Get-CimInstance Win32_Process -Filter "Name = 'eyeurai.exe'" -ErrorAction Stop
    foreach ($process in $processes) {
      if ($process.CommandLine -and
          $process.CommandLine.IndexOf($markerArgument, [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -and
          ((-not $process.ExecutablePath) -or
           [string]::Equals($process.ExecutablePath, $ExecutablePath, [System.StringComparison]::OrdinalIgnoreCase))) {
        Stop-SmokeProcess -ProcessId ([int64]$process.ProcessId)
      }
    }
  } catch {
    Write-Warning "Could not scan for an EyeUrAI smoke process during cleanup: $($_.Exception.Message)"
  }
}

function Stop-SmokeProcessesAtExecutable {
  param(
    [string]$ExecutablePath
  )

  if (-not $ExecutablePath) {
    return
  }

  # The installer target is unique to this smoke run. This final fallback also
  # catches a broken NSIS /ARGS handoff that launches the test executable but
  # drops the marker argument entirely.
  try {
    $processes = Get-CimInstance Win32_Process -Filter "Name = 'eyeurai.exe'" -ErrorAction Stop
    foreach ($process in $processes) {
      if ($process.ExecutablePath -and
          [string]::Equals($process.ExecutablePath, $ExecutablePath, [System.StringComparison]::OrdinalIgnoreCase)) {
        Stop-SmokeProcess -ProcessId ([int64]$process.ProcessId)
      }
    }
  } catch {
    Write-Warning "Could not scan the smoke install for an EyeUrAI process during cleanup: $($_.Exception.Message)"
  }
}

$executablePath = $null
$freshMarkerPath = $null
$freshApp = $null
$updateStartupMarkerPath = $null
$updateAppProcessId = [int64]0
$updateInstaller = $null
$hiddenMarkerPath = $null
$hiddenApp = $null
$ownsUpdateMarker = $false

try {
  $install = Start-SmokeProcess `
    -FilePath $installerPath `
    -Arguments @("/S", "/D=$installDir")
  $install.WaitForExit()
  if ($install.ExitCode -ne 0) {
    throw "EyeUrAI installer exited with code $($install.ExitCode)"
  }

  $executable = Get-ChildItem -LiteralPath $installDir -Filter "eyeurai.exe" -Recurse |
    Select-Object -First 1
  if (-not $executable) {
    throw "EyeUrAI executable was not installed under $installDir"
  }
  $executablePath = $executable.FullName
  Assert-WindowsGuiSubsystem -ExecutablePath $executablePath
  Write-Host "EyeUrAI's packaged executable uses the Windows GUI subsystem."

  # Preserve the original packaged-app bridge smoke test.
  $freshMarkerPath = New-StartupMarkerLease -Name "fresh"
  try {
    $freshApp = Start-SmokeProcess `
      -FilePath $executablePath `
      -Arguments @("--startup-smoke-marker=$freshMarkerPath")
    $freshProcessId = Wait-NativeBridgeReady `
      -MarkerPath $freshMarkerPath `
      -ExpectedProcessId $freshApp.Id
    Write-Host "EyeUrAI installed and its packaged frontend reached the native bridge (PID $freshProcessId)."
  } finally {
    if ($null -ne $freshApp) {
      Stop-SmokeProcess -ProcessId $freshApp.Id
    }
    Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $freshMarkerPath
  }

  if ((Test-Path -LiteralPath $updateMarkerPath) -or
      (Test-Path -LiteralPath $updateMarkerTempPath)) {
    throw "Refusing to start the update smoke test because a visibility marker appeared at $updateMarkerPath"
  }

  # Match the NSIS arguments emitted by tauri-plugin-updater. The installer
  # hook writes the visibility marker, while /ARGS deliberately carries the
  # inherited --hidden flag and this launch's unique PID marker to the app.
  $updateStartupMarkerPath = New-StartupMarkerLease -Name "update"
  $ownsUpdateMarker = $true
  try {
    $updateInstaller = Start-SmokeProcess `
      -FilePath $installerPath `
      -Arguments @(
        "/P",
        "/R",
        "/UPDATE",
        "/ARGS",
        "--hidden",
        "--startup-smoke-marker=$updateStartupMarkerPath"
      )

    # NSIS can spawn the replacement independently of the process handle
    # returned above. Trust only the native marker's exact app PID.
    $updateAppProcessId = Wait-NativeBridgeReady `
      -MarkerPath $updateStartupMarkerPath `
      -TimeoutSeconds 60
    Wait-MainWindowVisible -ProcessId $updateAppProcessId

    if (Test-Path -LiteralPath $updateMarkerPath) {
      throw "EyeUrAI showed after the update, but did not consume $updateMarkerPath"
    }
    if (Test-Path -LiteralPath $updateMarkerTempPath) {
      throw "The NSIS updater left its temporary visibility marker behind at $updateMarkerTempPath"
    }

    if (-not $updateInstaller.HasExited -and -not $updateInstaller.WaitForExit(15000)) {
      throw "The EyeUrAI update installer did not exit after relaunching the app."
    }
    if ($updateInstaller.HasExited -and $updateInstaller.ExitCode -ne 0) {
      throw "EyeUrAI update installer exited with code $($updateInstaller.ExitCode)"
    }

    # The marker is gone because the app consumed it, so later cleanup must not
    # remove a new marker that could be created by an unrelated user action.
    $ownsUpdateMarker = $false
    Write-Host "EyeUrAI's NSIS update relaunch ignored inherited --hidden, showed PID $updateAppProcessId, and consumed its visibility marker."
  } finally {
    Stop-SmokeProcess -ProcessId $updateAppProcessId
    Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $updateStartupMarkerPath
    Stop-SmokeProcessesAtExecutable -ExecutablePath $executablePath
    if ($null -ne $updateInstaller -and -not $updateInstaller.HasExited) {
      Stop-Process -Id $updateInstaller.Id -Force -ErrorAction SilentlyContinue
    }
  }

  if ((Test-Path -LiteralPath $updateMarkerPath) -or
      (Test-Path -LiteralPath $updateMarkerTempPath)) {
    throw "A visibility marker exists, so the plain --hidden launch cannot be tested safely."
  }

  $hiddenMarkerPath = New-StartupMarkerLease -Name "hidden"
  try {
    $hiddenApp = Start-SmokeProcess `
      -FilePath $executablePath `
      -Arguments @(
        "--hidden",
        "--startup-smoke-marker=$hiddenMarkerPath"
      )
    $hiddenProcessId = Wait-NativeBridgeReady `
      -MarkerPath $hiddenMarkerPath `
      -ExpectedProcessId $hiddenApp.Id
    Assert-MainWindowRemainsHidden -ProcessId $hiddenProcessId

    if (Test-Path -LiteralPath $updateMarkerPath) {
      throw "A plain --hidden launch unexpectedly created an update visibility marker."
    }
    Write-Host "EyeUrAI's plain --hidden launch reached the native bridge and kept PID $hiddenProcessId hidden."
  } finally {
    if ($null -ne $hiddenApp) {
      Stop-SmokeProcess -ProcessId $hiddenApp.Id
    }
    Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $hiddenMarkerPath
  }
} finally {
  if ($null -ne $freshApp) {
    Stop-SmokeProcess -ProcessId $freshApp.Id
  }
  Stop-SmokeProcess -ProcessId $updateAppProcessId
  if ($null -ne $hiddenApp) {
    Stop-SmokeProcess -ProcessId $hiddenApp.Id
  }

  Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $freshMarkerPath
  Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $updateStartupMarkerPath
  Stop-ProcessesForMarker -ExecutablePath $executablePath -MarkerPath $hiddenMarkerPath
  Stop-SmokeProcessesAtExecutable -ExecutablePath $executablePath

  if ($null -ne $updateInstaller -and -not $updateInstaller.HasExited) {
    Stop-Process -Id $updateInstaller.Id -Force -ErrorAction SilentlyContinue
  }

  if ($ownsUpdateMarker) {
    Remove-Item -LiteralPath $updateMarkerPath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $updateMarkerTempPath -Force -ErrorAction SilentlyContinue
  }
  Remove-Item -LiteralPath $markerDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
