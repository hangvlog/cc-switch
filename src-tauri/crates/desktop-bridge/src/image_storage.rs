//! Image history is durable; capacity pressure must never evict referenced originals.
use std::{fs, path::Path};

const MIB: u64 = 1024 * 1024;

pub(crate) fn ensure_capacity(root: &Path, additional: u64) -> Result<(), String> {
    let config = root.parent().ok_or("图片目录无效")?.join("image-storage.json");
    let limit_mib = match fs::read(&config) {
        Ok(bytes) => {
            let value: serde_json::Value = serde_json::from_slice(&bytes)
                .map_err(|_| "image-storage.json 格式无效")?;
            value["maxStorageMiB"].as_u64().filter(|v| (256..=65536).contains(v))
                .ok_or("maxStorageMiB 必须为 256 到 65536 的整数")?
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => 2048,
        Err(_) => return Err("无法读取图片容量配置".into()),
    };
    let mut used = 0_u64;
    for entry in fs::read_dir(root).map_err(|_| "无法读取图片目录")? {
        let entry = entry.map_err(|_| "无法读取图片目录项")?;
        let metadata = fs::symlink_metadata(entry.path()).map_err(|_| "无法读取图片大小")?;
        if metadata.is_file() {
            used = used.saturating_add(metadata.len());
        }
    }
    if used.saturating_add(additional) > limit_mib * MIB {
        return Err(format!("电脑图片存储已达到 {limit_mib} MiB 上限，请在电脑 image-storage.json 调整 maxStorageMiB 后重试；历史图片已保留"));
    }
    Ok(())
}
