<#
.SYNOPSIS
  Authenticode-sign files with signtool, using whichever signing material is configured.

.DESCRIPTION
  Tries, in order:
    1. A code-signing certificate file:   WINDOWS_CERTIFICATE (base64 .pfx), WINDOWS_CERTIFICATE_PASSWORD
    2. Azure Artifact Signing:            AZURE_SIGNING_ENDPOINT (e.g. https://neu.codesigning.azure.net),
                                          AZURE_SIGNING_ACCOUNT, AZURE_CERT_PROFILE, with the Azure CLI
                                          already signed in (`az login`; in CI, azure/login with OIDC)
  With neither, it prints a warning and leaves the files unsigned (exit code 0), so test builds
  still produce installers.

  This is the one place to adapt when the signing setup changes (an HSM/cloud key, a different
  timestamp server, ...). Optional overrides: WINDOWS_TIMESTAMP_URL, SIGNTOOL (path to signtool.exe).

.EXAMPLE
  pwsh packaging/windows/sign.ps1 dist/mailcraft.exe dist/Wordcraft.msi
#>
param(
  [Parameter(Mandatory = $true, ValueFromRemainingArguments = $true)]
  [string[]] $Files
)
$ErrorActionPreference = 'Stop'

function Write-Warn([string] $Message) {
  if ($env:GITHUB_ACTIONS) { Write-Output "::warning::$Message" } else { Write-Warning $Message }
}

function Find-SignTool {
  if ($env:SIGNTOOL) { return $env:SIGNTOOL }
  $onPath = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($onPath) { return $onPath.Source }
  $kits = Join-Path ${env:ProgramFiles(x86)} 'Windows Kits\10\bin'
  $found = Get-ChildItem -Path $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
    Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
    Sort-Object FullName -Descending | Select-Object -First 1
  if (-not $found) { throw 'signtool.exe not found (install the Windows SDK)' }
  return $found.FullName
}

function Invoke-SignTool([string[]] $Arguments) {
  & $script:SignTool @Arguments
  if ($LASTEXITCODE -ne 0) { throw "signtool $($Arguments[0]) failed with exit code $LASTEXITCODE" }
}

$Files = $Files | ForEach-Object { (Resolve-Path $_).Path }
$haveCert = [bool]$env:WINDOWS_CERTIFICATE
$azureVars = 'AZURE_SIGNING_ENDPOINT', 'AZURE_SIGNING_ACCOUNT', 'AZURE_CERT_PROFILE'
$haveAzure = -not ($azureVars | Where-Object { -not [Environment]::GetEnvironmentVariable($_) })

if (-not $haveCert -and -not $haveAzure) {
  Write-Warn "Windows signing secrets not set (WINDOWS_CERTIFICATE or AZURE_*): leaving unsigned: $($Files -join ', ')"
  exit 0
}

$script:SignTool = Find-SignTool
$common = @('sign', '/v', '/fd', 'SHA256', '/td', 'SHA256', '/d', 'MailCraft', '/du', 'https://github.com/UCS-Care-Group/mailcraft')
$tmp = Join-Path ([IO.Path]::GetTempPath()) "mailcraft-sign-$PID"
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
  if ($haveCert) {
    Write-Output "Signing with WINDOWS_CERTIFICATE: $($Files -join ', ')"
    $pfx = Join-Path $tmp 'cert.pfx'
    [IO.File]::WriteAllBytes($pfx, [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE))
    $ts = if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { 'http://timestamp.digicert.com' }
    $signArgs = $common + @('/tr', $ts, '/f', $pfx)
    if ($env:WINDOWS_CERTIFICATE_PASSWORD) { $signArgs += @('/p', $env:WINDOWS_CERTIFICATE_PASSWORD) }
    Invoke-SignTool ($signArgs + $Files)
  }
  else {
    Write-Output "Signing with Azure Artifact Signing ($env:AZURE_SIGNING_ACCOUNT / $env:AZURE_CERT_PROFILE): $($Files -join ', ')"
    # The dlib signs in through the Azure CLI's session alone (ExcludeCredentials below): no client
    # secret exists. In CI the session comes from azure/login with GitHub's OIDC token, which a
    # federated credential accepts only for the `release` environment.
    # The signing client is a pinned NuGet package, checked against this sha256 before it is
    # opened: it runs inside signtool with the Azure session.
    # Microsoft.ArtifactSigning.Client 1.0.146: https://www.nuget.org/packages/Microsoft.ArtifactSigning.Client/1.0.146
    $clientVersion = '1.0.146'
    $clientSha256 = '7761e5d8f31e54b764da21d33cd8b0a2940e1b05526c7dfcb5e94b0431426f70'
    $pkg = Join-Path $tmp 'artifact-signing.zip'
    Invoke-WebRequest -Uri "https://www.nuget.org/api/v2/package/Microsoft.ArtifactSigning.Client/$clientVersion" -OutFile $pkg
    $actual = (Get-FileHash -Algorithm SHA256 -Path $pkg).Hash.ToLowerInvariant()
    if ($actual -ne $clientSha256) {
      throw "Microsoft.ArtifactSigning.Client $clientVersion has sha256 $actual, expected $clientSha256"
    }
    Expand-Archive -Path $pkg -DestinationPath (Join-Path $tmp 'client') -Force
    $dlib = Get-ChildItem -Path (Join-Path $tmp 'client') -Recurse -Filter Azure.CodeSigning.Dlib.dll |
      Where-Object { $_.FullName -match '\\x64\\' } | Select-Object -First 1
    if (-not $dlib) { throw 'Azure.CodeSigning.Dlib.dll not found in Microsoft.ArtifactSigning.Client' }
    $metadata = Join-Path $tmp 'metadata.json'
    @{
      Endpoint               = $env:AZURE_SIGNING_ENDPOINT
      CodeSigningAccountName = $env:AZURE_SIGNING_ACCOUNT
      CertificateProfileName = $env:AZURE_CERT_PROFILE
      ExcludeCredentials     = @(
        'EnvironmentCredential', 'ManagedIdentityCredential', 'WorkloadIdentityCredential',
        'SharedTokenCacheCredential', 'VisualStudioCredential', 'VisualStudioCodeCredential',
        'AzurePowerShellCredential', 'AzureDeveloperCliCredential', 'InteractiveBrowserCredential'
      )
    } | ConvertTo-Json | Set-Content -Path $metadata -Encoding utf8
    $ts = if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { 'http://timestamp.acs.microsoft.com' }
    Invoke-SignTool ($common + @('/tr', $ts, '/dlib', $dlib.FullName, '/dmdf', $metadata) + $Files)
  }
  Invoke-SignTool (@('verify', '/pa', '/v') + $Files)
}
finally {
  Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
