$ErrorActionPreference = 'Stop'
$projectRoot = Split-Path -Parent $PSScriptRoot
$buildRoot = Join-Path $projectRoot '.build'
if (Test-Path -LiteralPath $buildRoot) { throw 'Use a clean .build directory before rebuilding assets.' }
New-Item -ItemType Directory -Force -Path $buildRoot | Out-Null
$download = Join-Path $buildRoot 'zapret.zip'
Invoke-WebRequest 'https://github.com/Flowseal/zapret-discord-youtube/releases/download/1.10.3/zapret-discord-youtube-1.10.3.zip' -OutFile $download
if ((Get-FileHash -LiteralPath $download -Algorithm SHA256).Hash -ne '244314AE1C24538A0D751601DA8E0C925C843371EEC4456EB15F14C4FD6B7058') { throw 'Upstream archive hash mismatch' }
$unpacked = Join-Path $buildRoot 'unpacked'
Expand-Archive -LiteralPath $download -DestinationPath $unpacked
$payload = Join-Path $buildRoot 'payload'
New-Item -ItemType Directory -Force -Path $payload | Out-Null
Copy-Item -LiteralPath (Join-Path $unpacked 'zapret-discord-youtube-1.10.3') -Destination (Join-Path $payload 'zapret') -Recurse
$licenses = Join-Path $payload 'licenses'
New-Item -ItemType Directory -Force -Path $licenses | Out-Null
Invoke-WebRequest 'https://raw.githubusercontent.com/valnesfjord/tg-ws-proxy-rs/53f392280a370199ffe5c6ef0ac88808ed17c575/LICENSE' -OutFile (Join-Path $licenses 'tg-ws-proxy-rs-MIT.txt')
Invoke-WebRequest 'https://raw.githubusercontent.com/Flowseal/tg-ws-proxy/9f71904e1a4a282fd9e5044bbb71d59f4d30576b/LICENSE' -OutFile (Join-Path $licenses 'tg-ws-proxy-Flowseal-MIT.txt')
Invoke-WebRequest 'https://raw.githubusercontent.com/basil00/WinDivert/master/LICENSE' -OutFile (Join-Path $payload 'zapret/LICENSE-WinDivert.txt')
$lgpl = (Invoke-WebRequest 'https://www.gnu.org/licenses/lgpl-3.0.txt').Content
$gpl = (Invoke-WebRequest 'https://www.gnu.org/licenses/gpl-3.0.txt').Content
Set-Content -LiteralPath (Join-Path $payload 'zapret/LICENSE-Cygwin.txt') -Value "Cygwin runtime 3.4.10. Licensing: https://cygwin.com/licensing.html`n`n$lgpl`n`n$gpl" -Encoding utf8NoBOM
Copy-Item -LiteralPath (Join-Path $projectRoot 'docs/RUST_LICENSES.txt') -Destination (Join-Path $licenses 'RUST_LICENSES.txt')
Invoke-WebRequest 'https://raw.githubusercontent.com/ImMALWARE/dns.malw.link/062b1a8687e6c2606c0bd4b06e833e65cfaefb29/LICENSE' -OutFile (Join-Path $licenses 'dns.malw.link-MIT.txt')
$manifest = [ordered]@{telegram_rust_commit='53f392280a370199ffe5c6ef0ac88808ed17c575';zapret_version='1.10.3';files=[ordered]@{}}
$archive = Join-Path $projectRoot 'assets/engines.zip'
$stream = [System.IO.File]::Create($archive)
$zip = [System.IO.Compression.ZipArchive]::new($stream,[System.IO.Compression.ZipArchiveMode]::Create)
try {
    foreach ($file in (Get-ChildItem -LiteralPath $payload -File -Recurse | Sort-Object FullName)) {
        $relative = [System.IO.Path]::GetRelativePath($payload,$file.FullName).Replace('\','/')
        $manifest.files[$relative] = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        [System.IO.Compression.ZipFileExtensions]::CreateEntryFromFile($zip,$file.FullName,$relative,[System.IO.Compression.CompressionLevel]::Optimal) | Out-Null
    }
} finally { $zip.Dispose(); $stream.Dispose() }
$manifest.bundle_sha256 = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash.ToLowerInvariant()
$manifest | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath (Join-Path $projectRoot 'assets/manifest.json') -Encoding utf8NoBOM
Write-Host 'Embedded assets rebuilt. Run cargo test and cargo build --release.'
