# Instala Huella en Windows: descarga el binario de la última release,
# comprueba su checksum, lo deja en %LOCALAPPDATA%\Programs\huella (en el
# PATH del usuario) y lanza `huella setup`.
#
#   irm https://raw.githubusercontent.com/juligarc10/huella/main/install.ps1 | iex
#
# Variables opcionales:
#   HUELLA_VERSION       versión a instalar (p. ej. v0.1.0; por defecto la última)
#   HUELLA_INSTALL_DIR   carpeta del binario
#   HUELLA_NO_SETUP=1    no lanza `huella setup` al terminar
#   HUELLA_DOWNLOAD_URL  base de descarga alternativa (un espejo)

$ErrorActionPreference = 'Stop'
$Repo = 'https://github.com/juligarc10/huella'
# Windows PowerShell 5.1 puede negociar un TLS que GitHub ya no acepta.
[Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

# Con `irm … | iex` el script corre en la sesión de quien lo lanza: un error
# detiene la instalación sin cerrarle la ventana, como haría `exit`.
function Fail($message) {
    throw "huella: $message"
}

function Get-Target {
    # Windows en ARM ejecuta binarios x64 con emulación.
    $arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
    if ($arch -ne 'X64' -and $arch -ne 'Arm64') { Fail "arquitectura no soportada: $arch" }
    'x86_64-pc-windows-msvc'
}

# Última versión publicada.
function Get-LatestVersion {
    try {
        $release = Invoke-RestMethod -UseBasicParsing 'https://api.github.com/repos/juligarc10/huella/releases/latest'
    } catch { Fail "no se pudo consultar la última versión en $Repo/releases" }
    if ($release.tag_name -notmatch '^v[0-9]') { Fail 'no hay ninguna release publicada todavía' }
    $release.tag_name
}

$target = Get-Target
$version = if ($env:HUELLA_VERSION) { $env:HUELLA_VERSION } else { Get-LatestVersion }
$base = if ($env:HUELLA_DOWNLOAD_URL) { $env:HUELLA_DOWNLOAD_URL } else { "$Repo/releases/download" }
$installDir = if ($env:HUELLA_INSTALL_DIR) { $env:HUELLA_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA 'Programs\huella' }
$archive = "huella-$version-$target.zip"

$work = Join-Path ([System.IO.Path]::GetTempPath()) ("huella-" + [System.Guid]::NewGuid())
New-Item -ItemType Directory $work | Out-Null
try {
    Write-Host "Descargando Huella $version para $target…"
    try {
        Invoke-WebRequest -UseBasicParsing "$base/$version/$archive" -OutFile (Join-Path $work $archive)
    } catch { Fail "no se pudo descargar $base/$version/$archive" }
    try {
        Invoke-WebRequest -UseBasicParsing "$base/$version/SHA256SUMS" -OutFile (Join-Path $work 'SHA256SUMS')
    } catch { Fail "la release $version no publica SHA256SUMS: no se instala sin comprobarla" }

    $expected = Get-Content (Join-Path $work 'SHA256SUMS') |
        ForEach-Object { $parts = $_ -split '\s+'; if ($parts[1] -eq $archive -or $parts[1] -eq "*$archive") { $parts[0] } } |
        Select-Object -First 1
    if (-not $expected) { Fail "SHA256SUMS no incluye $archive" }
    $actual = (Get-FileHash -Algorithm SHA256 (Join-Path $work $archive)).Hash
    if ($actual.ToLowerInvariant() -ne $expected.ToLowerInvariant()) {
        Fail "el checksum de $archive no coincide: la descarga está corrupta o la han cambiado"
    }
    Write-Host 'Checksum comprobado.'

    Expand-Archive -Path (Join-Path $work $archive) -DestinationPath $work -Force
    New-Item -ItemType Directory -Force $installDir | Out-Null
    Copy-Item (Join-Path $work "huella-$version-$target\huella.exe") (Join-Path $installDir 'huella.exe') -Force
    Write-Host "Instalado en $installDir\huella.exe"
} finally {
    Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
}

# En el PATH del usuario, para las terminales y los editores que se abran después.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not (($userPath -split ';') -contains $installDir)) {
    $newPath = if ($userPath) { "$userPath;$installDir" } else { $installDir }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$installDir"
    Write-Host "Añadido $installDir a tu PATH (abre una terminal nueva para que lo vean las demás)."
}

if ($env:HUELLA_NO_SETUP -eq '1') {
    Write-Host "Para terminar de configurarlo: huella setup"
} else {
    Write-Host ''
    & (Join-Path $installDir 'huella.exe') setup
}
