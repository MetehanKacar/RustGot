# Windows version of build-jar.sh: builds patchbukkit.jar and checks it against the SHA-256
# pinned in ..\src\java\bridge_jar.rs. Needs a JDK 25 (JAVA_HOME) and network access.
# Pass -SkipPin to accept a different hash (when updating the pin).
param([switch]$SkipPin)
$ErrorActionPreference = 'Stop'

$GradleVersion = '9.8.1'
$GradleSha256 = 'dce76f55f8e251a3a1f130eb120f30b3d271de2b76c9b0729d316b5a1b6dc01f'

$here = $PSScriptRoot
$cache = if ($env:GRADLE_USER_HOME) { $env:GRADLE_USER_HOME } else { Join-Path $HOME '.gradle' }
$dist = Join-Path $cache "rustgot\gradle-$GradleVersion"
$gradle = Join-Path $dist 'bin\gradle.bat'

if (-not (Test-Path $gradle)) {
    New-Item -ItemType Directory -Force (Join-Path $cache 'rustgot') | Out-Null
    $zip = Join-Path $cache "rustgot\gradle-$GradleVersion-bin.zip"
    Invoke-WebRequest -Uri "https://services.gradle.org/distributions/gradle-$GradleVersion-bin.zip" -OutFile $zip
    $hash = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
    if ($hash -ne $GradleSha256) { Remove-Item $zip; throw "Gradle SHA-256 mismatch: $hash" }
    Expand-Archive -Force $zip (Join-Path $cache 'rustgot')
    Remove-Item $zip
}

& $gradle -p $here --no-daemon --console=plain :patchbukkit:jar
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$jar = Join-Path $here 'patchbukkit\build\libs\patchbukkit.jar'
$pinned = (Select-String -Path (Join-Path $here '..\src\java\bridge_jar.rs') -Pattern 'pub const RELEASE_SHA256: &str = "([0-9a-f]+)";').Matches[0].Groups[1].Value
$actual = (Get-FileHash -Algorithm SHA256 $jar).Hash.ToLower()
Write-Output "$actual  $jar"
if ($actual -ne $pinned -and -not $SkipPin) { throw "SHA-256 differs from the pinned $pinned" }
