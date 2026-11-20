use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub const INTERVAL: Duration = Duration::from_secs(30 * 60);
const REPO: &str = "Absurd1k/relay";
const MAX_EXE: u64 = 128 * 1024 * 1024;
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Download {
    pub version: String,
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug)]
pub enum Status {
    Checking,
    Downloading {
        version: String,
        bytes: u64,
        total: u64,
    },
    Ready(Download),
    Current,
    Skipped,
    Error(String),
}
#[derive(Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    pub assets: Vec<Asset>,
}
#[derive(Deserialize)]
pub struct Asset {
    pub name: String,
    pub browser_download_url: String,
    pub size: u64,
    pub digest: Option<String>,
}
fn version(tag: &str) -> Option<semver::Version> {
    semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).ok()
}
pub fn select(releases: Vec<Release>, current: &str) -> Option<Release> {
    let current = version(current)?;
    releases
        .into_iter()
        .filter(|r| {
            !r.draft
                && r.assets.iter().any(|a| a.name == "Relay.exe")
                && version(&r.tag_name).is_some_and(|v| v > current)
        })
        .max_by_key(|r| version(&r.tag_name))
}
fn get(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(25)))
        .build()
        .into();
    let response = agent
        .get(url)
        .header("User-Agent", "Relay-Windows-Updater")
        .header("Accept", "application/atom+xml,text/plain,*/*")
        .call()
        .map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    response
        .into_body()
        .into_reader()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Слишком большой ответ GitHub".into());
    }
    Ok(bytes)
}
fn atom_releases(bytes: &[u8]) -> Result<Vec<Release>, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Некорректная кодировка ленты GitHub")?;
    if !text.contains("<feed ") {
        return Err("GitHub вернул некорректную ленту релизов".into());
    }
    let prefix = format!("href=\"https://github.com/{REPO}/releases/tag/");
    let mut releases = vec![];
    // GitHub escapes release-note HTML. Read only actual link elements, never titles/content.
    for entry in text.split("<entry>").skip(1).take(50) {
        let entry = entry.split("</entry>").next().unwrap_or("");
        for link in entry.split("<link ").skip(1) {
            let Some((attributes, _)) = link.split_once('>') else {
                continue;
            };
            let Some((_, tail)) = attributes.split_once(&prefix) else {
                continue;
            };
            let Some((tag, _)) = tail.split_once('"') else {
                continue;
            };
            if version(tag).is_none()
                || !tag
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c))
            {
                continue;
            }
            releases.push(Release {
                tag_name: tag.into(),
                draft: false,
                assets: vec![Asset {
                    name: "Relay.exe".into(),
                    browser_download_url: format!(
                        "https://github.com/{REPO}/releases/download/{tag}/Relay.exe"
                    ),
                    size: 0,
                    digest: None,
                }],
            });
            break;
        }
    }
    if releases.is_empty() {
        return Err("В ленте GitHub нет поддерживаемых релизов".into());
    }
    Ok(releases)
}
fn checksum(bytes: &[u8]) -> Result<String, String> {
    let text = std::str::from_utf8(bytes).map_err(|_| "Некорректный файл SHA256SUMS.txt")?;
    let mut found = None;
    for line in text.lines() {
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() == 2 && fields[1].trim_start_matches('*') == "Relay.exe" {
            if !valid_hash(fields[0]) || found.is_some() {
                return Err("Некорректная или повторная контрольная сумма Relay.exe".into());
            }
            found = Some(fields[0].to_ascii_lowercase());
        }
    }
    found.ok_or_else(|| "В релизе нет контрольной суммы Relay.exe".into())
}
fn valid_hash(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|b| b.is_ascii_hexdigit())
}
pub fn hash_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 32768];
    loop {
        let n = f.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(format!("{:x}", h.finalize()))
}
pub fn verify_exe(path: &Path) -> Result<(), String> {
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let mut head = [0u8; 64];
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if &head[..2] != b"MZ" {
        return Err("Обновление не является Windows exe".into());
    }
    let offset = u32::from_le_bytes(head[60..64].try_into().unwrap()) as u64;
    if offset > len.saturating_sub(6) {
        return Err("Повреждён PE-заголовок".into());
    }
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut pe = [0u8; 6];
    f.read_exact(&mut pe).map_err(|e| e.to_string())?;
    if &pe[..4] != b"PE\0\0" || u16::from_le_bytes([pe[4], pe[5]]) != 0x8664 {
        return Err("Обновление не предназначено для Windows x64".into());
    }
    Ok(())
}
pub fn verify_download(d: &Download) -> Result<(), String> {
    if !valid_hash(&d.sha256)
        || std::fs::metadata(&d.path).map_err(|e| e.to_string())?.len() != d.size
        || hash_file(&d.path)? != d.sha256
    {
        return Err("Контрольная сумма обновления не совпала".into());
    }
    verify_exe(&d.path)
}
fn cached(data: &Path) -> Option<Download> {
    let d: Download =
        serde_json::from_slice(&std::fs::read(data.join("updates/ready.json")).ok()?).ok()?;
    let ver = version(&d.version)?;
    if ver <= version(env!("CARGO_PKG_VERSION"))? {
        return None;
    }
    let expected = data.join("updates").join(ver.to_string()).join("Relay.exe");
    if d.path != expected {
        return None;
    }
    verify_download(&d).ok()?;
    Some(d)
}
pub fn check(data: &Path, force: bool, emit: impl Fn(Status)) -> Result<(), String> {
    check_for_version(data, force, env!("CARGO_PKG_VERSION"), emit)
}
fn check_for_version(
    data: &Path,
    force: bool,
    current: &str,
    emit: impl Fn(Status),
) -> Result<(), String> {
    if let Some(d) = cached(data) {
        emit(Status::Ready(d));
        return Ok(());
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let stamp = data.join("updates/last-check");
    let last = std::fs::read_to_string(&stamp)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    if !force && now.saturating_sub(last) < INTERVAL.as_secs() {
        emit(Status::Skipped);
        return Ok(());
    }
    emit(Status::Checking);
    let releases = atom_releases(&get(
        &format!("https://github.com/{REPO}/releases.atom"),
        1024 * 1024,
    )?)?;
    std::fs::create_dir_all(stamp.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(&stamp, now.to_string()).map_err(|e| e.to_string())?;
    let Some(release) = select(releases, current) else {
        emit(Status::Current);
        return Ok(());
    };
    let ver = version(&release.tag_name).ok_or("Некорректная версия")?;
    if !release
        .tag_name
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b".-+".contains(&c))
    {
        return Err("Неподдерживаемый тег релиза".into());
    }
    let asset = release
        .assets
        .into_iter()
        .find(|a| a.name == "Relay.exe")
        .ok_or("В релизе нет Relay.exe")?;
    let expected = format!(
        "https://github.com/{REPO}/releases/download/{}/Relay.exe",
        release.tag_name
    );
    if asset.browser_download_url != expected
        || (asset.size != 0 && asset.size < 512)
        || asset.size > MAX_EXE
    {
        return Err("Недопустимый адрес или размер обновления".into());
    }
    let hash = checksum(&get(
        &format!(
            "https://github.com/{REPO}/releases/download/{}/SHA256SUMS.txt",
            release.tag_name
        ),
        64 * 1024,
    )?)?;
    let dir = data.join("updates").join(ver.to_string());
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let temp = dir.join("Relay.exe.part");
    let final_path = dir.join("Relay.exe");
    emit(Status::Downloading {
        version: ver.to_string(),
        bytes: 0,
        total: asset.size,
    });
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(180)))
        .build()
        .into();
    let response = agent
        .get(&asset.browser_download_url)
        .header("User-Agent", "Relay-Windows-Updater")
        .call()
        .map_err(|e| e.to_string())?;
    let expected_size = response
        .headers()
        .get("content-length")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok());
    if expected_size.is_some_and(|n| !(512..=MAX_EXE).contains(&n)) {
        return Err("Недопустимый размер обновления".into());
    }
    let limit = expected_size.unwrap_or(MAX_EXE);
    let mut reader = response.into_body().into_reader().take(limit + 1);
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    let mut buffer = [0u8; 32768];
    let mut size = 0u64;
    let mut last_progress = std::time::Instant::now();
    loop {
        let n = reader.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        size += n as u64;
        if size > limit {
            return Err("Размер скачанного обновления превышен".into());
        }
        file.write_all(&buffer[..n]).map_err(|e| e.to_string())?;
        if last_progress.elapsed() > Duration::from_millis(300) {
            emit(Status::Downloading {
                version: ver.to_string(),
                bytes: size,
                total: expected_size.unwrap_or(0),
            });
            last_progress = std::time::Instant::now();
        }
    }
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    let mut d = Download {
        version: ver.to_string(),
        path: temp,
        sha256: hash,
        size,
    };
    if size < 512 || expected_size.is_some_and(|n| n != size) {
        return Err("Обновление скачано не полностью".into());
    }
    verify_download(&d)?;
    std::fs::rename(&d.path, &final_path).map_err(|e| e.to_string())?;
    d.path = final_path;
    std::fs::write(
        data.join("updates/ready.json"),
        serde_json::to_vec(&d).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    emit(Status::Ready(d));
    Ok(())
}
fn literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}
pub fn install_script(d: &Download, target: &Path, pid: u32, log: &Path) -> String {
    format!(
        r#"$ErrorActionPreference='Stop'
$sourcePath={}
$target={}
$expected={}
$log={}
try{{
 $parent=$null
 try{{$parent=[Diagnostics.Process]::GetProcessById({pid})}}catch{{}}
 if($null -ne $parent -and -not $parent.WaitForExit(45000)){{throw 'Relay did not stop; update cancelled'}}
 $source=[IO.File]::Open($sourcePath,[IO.FileMode]::Open,[IO.FileAccess]::Read,[IO.FileShare]::Read)
 try{{
  $sha=[Security.Cryptography.SHA256]::Create()
  $actual=([BitConverter]::ToString($sha.ComputeHash($source))).Replace('-','').ToLowerInvariant()
  if($actual -ne $expected){{throw 'SHA256 mismatch'}}
  $source.Position=0
  $temp=$target+'.new-'+[Guid]::NewGuid().ToString('N')
  $out=[IO.File]::Open($temp,[IO.FileMode]::CreateNew,[IO.FileAccess]::Write,[IO.FileShare]::None)
  try{{$source.CopyTo($out);$out.Flush($true)}}finally{{$out.Dispose()}}
 }}finally{{$source.Dispose()}}
 $backup=$target+'.old-'+[Guid]::NewGuid().ToString('N')
 [IO.File]::Replace($temp,$target,$backup)
 try{{
  $app=Start-Process -FilePath $target -ArgumentList '--resume-after-update' -WorkingDirectory ([IO.Path]::GetDirectoryName($target)) -PassThru
  Start-Sleep -Seconds 3
  if($app.HasExited){{throw 'New Relay exited during startup'}}
  [IO.File]::Delete($backup)
 }}catch{{[IO.File]::Replace($backup,$target,$null);Start-Process -FilePath $target -ArgumentList '--resume-after-update';throw}}
}}catch{{[IO.File]::WriteAllText($log,$_.Exception.Message)}}
"#,
        literal(&d.path.to_string_lossy()),
        literal(&target.to_string_lossy()),
        literal(&d.sha256),
        literal(&log.to_string_lossy())
    )
}
pub fn install(d: &Download, data: &Path) -> Result<(), String> {
    verify_download(d)?;
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let script = install_script(
        d,
        &target,
        std::process::id(),
        &data.join("updates/install-error.txt"),
    );
    let shell =
        PathBuf::from(std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into()))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
    let mut command = std::process::Command::new(shell);
    command
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &script,
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    crate::platform::hidden(&mut command)
        .spawn()
        .map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn atom_links_and_checksum_are_restricted_to_our_release_asset() {
        let atom=br#"<feed xmlns="http://www.w3.org/2005/Atom"><entry><title>v99.0.0</title><link href="https://evil.example/releases/tag/v99.0.0"/><link href="https://github.com/Absurd1k/relay/releases/tag/v0.2.1"/></entry><entry><link href="https://github.com/Absurd1k/relay/releases/tag/../../bad"/></entry></feed>"#;
        let releases = atom_releases(atom).unwrap();
        assert_eq!(releases.len(), 1);
        assert_eq!(releases[0].tag_name, "v0.2.1");
        let hash = "a".repeat(64);
        assert_eq!(
            checksum(
                format!("{}  Other.exe\r\n{} *Relay.exe\r\n", "b".repeat(64), hash).as_bytes()
            )
            .unwrap(),
            hash
        );
        assert!(checksum(b"bad  Relay.exe").is_err());
        assert!(checksum(format!("{hash} Relay.exe\n{hash} Relay.exe").as_bytes()).is_err());
    }
    #[test]
    #[ignore = "downloads a public release from GitHub; never executes it"]
    fn live_github_download_verifies_public_release_without_api() {
        let dir = std::env::temp_dir().join(format!(
            "relay-live-update-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let ready = std::sync::Mutex::new(None);
        check_for_version(&dir, true, "0.0.0", |s| {
            if let Status::Ready(d) = s {
                *ready.lock().unwrap() = Some(d);
            }
        })
        .unwrap();
        let download = ready.into_inner().unwrap().expect("published release");
        verify_download(&download).unwrap();
        assert!(download.size >= 512);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn downloaded_executable_rejects_tampering_and_wrong_architecture() {
        let dir = std::env::temp_dir().join(format!("relay-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Relay.exe");
        let mut bytes = vec![0u8; 512];
        bytes[..2].copy_from_slice(b"MZ");
        bytes[60..64].copy_from_slice(&64u32.to_le_bytes());
        bytes[64..70].copy_from_slice(b"PE\0\0\x64\x86");
        std::fs::write(&path, &bytes).unwrap();
        let d = Download {
            version: "9.0.0".into(),
            path: path.clone(),
            sha256: hash_file(&path).unwrap(),
            size: 512,
        };
        assert!(verify_download(&d).is_ok());
        bytes[100] = 1;
        std::fs::write(&path, &bytes).unwrap();
        assert!(verify_download(&d).is_err());
        bytes[68..70].copy_from_slice(&0x14cu16.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();
        assert!(verify_exe(&path).is_err());
        std::fs::remove_file(path).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }
    #[test]
    fn newest_published_asset_wins_even_for_prereleases() {
        let json = r#"[{"tag_name":"v0.3.0","draft":true,"assets":[{"name":"Relay.exe","browser_download_url":"","size":1000,"digest":null}]},{"tag_name":"v0.2.1-beta.1","assets":[{"name":"Relay.exe","browser_download_url":"","size":1000,"digest":null}]},{"tag_name":"v0.2.0","assets":[{"name":"Relay.exe","browser_download_url":"","size":1000,"digest":null}]}]"#;
        assert_eq!(
            select(serde_json::from_str(json).unwrap(), "0.2.0")
                .unwrap()
                .tag_name,
            "v0.2.1-beta.1"
        );
    }
    #[test]
    fn installer_escapes_paths_and_never_kills_the_parent() {
        let d = Download {
            version: "0.3.0".into(),
            path: "C:\\O'Brien\\Relay.exe".into(),
            sha256: "a".repeat(64),
            size: 1000,
        };
        let script = install_script(
            &d,
            Path::new("C:\\My App\\Relay.exe"),
            42,
            Path::new("C:\\error.txt"),
        );
        assert!(script.contains("O''Brien"));
        assert!(script.contains("WaitForExit(45000)"));
        assert!(!script.contains("Stop-Process"));
        assert!(script.contains("ComputeHash($source)"));
        assert!(script.contains("[IO.File]::Replace"));
    }
    #[cfg(windows)]
    #[test]
    fn installer_replaces_atomically_and_leaves_original_on_checksum_failure() {
        let dir = std::env::temp_dir().join(format!("Relay updater's test {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("download.exe");
        let target = dir.join("Relay.exe");
        let log = dir.join("error.txt");
        std::fs::write(&source, b"new version").unwrap();
        std::fs::write(&target, b"old version").unwrap();
        let mut d = Download {
            version: "9.0.0".into(),
            path: source.clone(),
            sha256: hash_file(&source).unwrap(),
            size: 11,
        };
        // Only process launch and the three-second delay are mocked; real File.Replace and hashing run.
        let run = |d: &Download| {
            let script = format!(
                "function Start-Process {{ [pscustomobject]@{{HasExited=$false}} }}\nfunction Start-Sleep {{}}\n{}",
                install_script(d, &target, u32::MAX, &log)
            );
            let output = std::process::Command::new("powershell.exe")
                .args(["-NoProfile", "-NonInteractive", "-Command", &script])
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        };
        run(&d);
        assert_eq!(std::fs::read(&target).unwrap(), b"new version");
        assert!(!log.exists());
        d.sha256 = "0".repeat(64);
        std::fs::write(&target, b"keep this version").unwrap();
        run(&d);
        assert_eq!(std::fs::read(&target).unwrap(), b"keep this version");
        assert!(
            std::fs::read_to_string(&log)
                .unwrap()
                .contains("SHA256 mismatch")
        );
        for p in [source, target, log] {
            std::fs::remove_file(p).unwrap();
        }
        std::fs::remove_dir(dir).unwrap();
    }
}
