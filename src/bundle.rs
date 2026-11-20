use sha2::{Digest, Sha256};
use std::{
    io::{Cursor, Read},
    path::{Path, PathBuf},
};

pub const PAYLOAD: &[u8] = include_bytes!("../assets/engines.zip");
pub const MANIFEST: &str = include_str!("../assets/manifest.json");

#[derive(serde::Deserialize)]
pub struct Manifest {
    pub bundle_sha256: String,
    pub files: std::collections::BTreeMap<String, String>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn safe_entry(name: &str) -> Result<PathBuf, String> {
    let normalized = name.replace('\\', "/");
    if normalized.starts_with('/')
        || normalized.contains(':')
        || normalized.split('/').any(|p| p == "..")
    {
        return Err(format!("Небезопасный путь в архиве: {name}"));
    }
    Ok(PathBuf::from(normalized))
}

pub fn unpack(bytes: &[u8], dest: &Path) -> Result<(), String> {
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut total = 0u64;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
        let rel = safe_entry(entry.name())?;
        if entry.is_symlink() {
            return Err("Символические ссылки в архиве запрещены".into());
        }
        total = total
            .checked_add(entry.size())
            .ok_or("Слишком большой архив")?;
        if total > 512 * 1024 * 1024 {
            return Err("Слишком большой архив".into());
        }
        let path = dest.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(path).map_err(|e| e.to_string())?;
            continue;
        }
        std::fs::create_dir_all(path.parent().ok_or("Нет родительской папки")?)
            .map_err(|e| e.to_string())?;
        let mut out = std::fs::File::create(path).map_err(|e| e.to_string())?;
        std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn verify(root: &Path, manifest: &Manifest) -> Result<(), String> {
    for (name, expected) in &manifest.files {
        let path = root.join(safe_entry(name)?);
        let mut file =
            std::fs::File::open(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut hash = Sha256::new();
        let mut buf = [0u8; 65536];
        loop {
            let n = file.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            hash.update(&buf[..n]);
        }
        if format!("{:x}", hash.finalize()) != *expected {
            return Err(format!(
                "Изменён встроенный файл {name}. Удалите Relay_data/engines и перезапустите приложение."
            ));
        }
    }
    Ok(())
}

pub fn prepare(data: &Path) -> Result<PathBuf, String> {
    let manifest: Manifest = serde_json::from_str(MANIFEST).map_err(|e| e.to_string())?;
    if digest(PAYLOAD) != manifest.bundle_sha256 {
        return Err("Повреждён встроенный архив".into());
    }
    let cache_id = &manifest.bundle_sha256[..16];
    let root = data.join("engines").join(cache_id);
    if !root.exists() {
        let stage = data.join(format!("engines-staging-{cache_id}"));
        if stage.exists() {
            return Err("Осталась папка незавершённой распаковки Relay_data/engines-staging. Удалите её и повторите запуск.".into());
        }
        std::fs::create_dir_all(&stage)
            .map_err(|e| format!("Не удалось создать папку рядом с Relay.exe: {e}"))?;
        unpack(PAYLOAD, &stage)?;
        verify(&stage, &manifest)?;
        std::fs::create_dir_all(root.parent().ok_or("Нет папки компонентов")?)
            .map_err(|e| e.to_string())?;
        std::fs::rename(&stage, &root).map_err(|e| e.to_string())?;
    }
    verify(&root, &manifest)?;
    Ok(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_zip_slip_on_windows_and_unix() {
        for name in [
            "../x",
            "a/../../x",
            "a\\..\\x",
            "C:\\x",
            "/x",
            "\\\\server\\x",
        ] {
            assert!(safe_entry(name).is_err(), "{name}");
        }
        assert!(safe_entry("zapret/bin/winws.exe").is_ok());
    }
    #[test]
    fn bundled_archive_matches_manifest() {
        let m: Manifest = serde_json::from_str(MANIFEST).unwrap();
        assert_eq!(digest(PAYLOAD), m.bundle_sha256);
        let mut archive = zip::ZipArchive::new(Cursor::new(PAYLOAD)).unwrap();
        for (path, hash) in &m.files {
            let mut bytes = Vec::new();
            archive
                .by_name(path)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            assert_eq!(&digest(&bytes), hash, "{path}");
        }
        assert!(!m.files.keys().any(|p| p.starts_with("telegram/")));
        assert!(m.files.contains_key("zapret/bin/winws.exe"));
    }
}
