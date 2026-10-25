# Dot-source this script to install the MSVC GTK stack and configure the current shell.
param(
    [string]$GtkRoot = (Join-Path $env:LOCALAPPDATA 'Lantern\gtk\2026.8.0')
)

$ErrorActionPreference = 'Stop'
$gtkVersion = '2026.8.0'
$gtkSha256 = '1f95a92d037f5292da05e6ab1037032ff21ddb7b20d4ac8e83e3674c864c07b0'
$gtkMarker = Join-Path $GtkRoot '.lantern-gtk-sha256'

if (!(Test-Path $gtkMarker) -or (Get-Content $gtkMarker -Raw).Trim() -ne $gtkSha256) {
    $gtkArchive = Join-Path ([System.IO.Path]::GetTempPath()) "GTK4_Gvsbuild_${gtkVersion}_x64.zip"
    if (!(Test-Path $gtkArchive) -or (Get-FileHash $gtkArchive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $gtkSha256) {
        Invoke-WebRequest "https://github.com/wingtk/gvsbuild/releases/download/$gtkVersion/GTK4_Gvsbuild_${gtkVersion}_x64.zip" -OutFile $gtkArchive
    }
    if ((Get-FileHash $gtkArchive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $gtkSha256) {
        throw 'The GTK download failed SHA-256 verification.'
    }
    New-Item -ItemType Directory -Force -Path $GtkRoot | Out-Null
    Expand-Archive -Path $gtkArchive -DestinationPath $GtkRoot -Force
    Set-Content -Path $gtkMarker -Value $gtkSha256
}

$GtkRoot = (Resolve-Path $GtkRoot).Path
$env:Path = "$GtkRoot\bin;$env:Path"
$env:LIB = "$GtkRoot\lib;$env:LIB"
$env:INCLUDE = "$GtkRoot\include;$env:INCLUDE"
$env:PKG_CONFIG_PATH = "$GtkRoot\lib\pkgconfig;$GtkRoot\share\pkgconfig;$env:PKG_CONFIG_PATH"
# gvsbuild's .pc files contain the prefix of the machine that built the archive.
Get-ChildItem "$GtkRoot\lib\pkgconfig", "$GtkRoot\share\pkgconfig" -Filter '*.pc' -ErrorAction SilentlyContinue | ForEach-Object {
    $content = [System.IO.File]::ReadAllText($_.FullName)
    $content = [regex]::Replace($content, '(?m)^prefix=.*$', "prefix=$($GtkRoot.Replace('\', '/'))")
    [System.IO.File]::WriteAllText($_.FullName, $content)
}
$env:PKG_CONFIG = Join-Path $GtkRoot 'bin\pkgconf.exe'

& $env:PKG_CONFIG --atleast-version=4.22 gtk4
if ($LASTEXITCODE -ne 0) { throw 'GTK 4.22 or newer was not found.' }
& $env:PKG_CONFIG --atleast-version=1.56 pango
if ($LASTEXITCODE -ne 0) { throw 'Pango 1.56 or newer was not found.' }
& $env:PKG_CONFIG --exists gtksourceview-5
if ($LASTEXITCODE -ne 0) { throw 'GtkSourceView 5 was not found.' }

Write-Host "GTK is ready in $GtkRoot"
