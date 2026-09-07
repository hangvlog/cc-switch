use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use zip::write::SimpleFileOptions;

const MAX_TEXT_BYTES: usize = 512 * 1024;
const MAX_BUNDLE_BYTES: usize = 2 * 1024 * 1024;
static BEARER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(bearer\s+)[A-Za-z0-9._~+/=-]{8,}").expect("valid bearer regex")
});
static API_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bsk-[A-Za-z0-9_-]{8,}\b").expect("valid key regex"));
static JWT_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\beyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}\b")
        .expect("valid jwt regex")
});

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticBundleUpload {
    pub bundle_id: String,
    pub url: String,
    pub expires_at: i64,
    pub expires_in_seconds: i64,
}

#[derive(Deserialize)]
struct UploadEnvelope {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    detail: String,
    data: Option<UploadData>,
}

#[derive(Deserialize)]
struct UploadData {
    bundle_id: String,
    url: String,
    expires_at: i64,
    expires_in_seconds: i64,
}

pub async fn create_and_upload() -> Result<DiagnosticBundleUpload, String> {
    let account = crate::clawkit_account::ClawkitAccountClient::default();
    let (endpoint, account_token) = account.diagnostics_upload_context()?;
    let payload = build_bundle(
        &crate::codex_config::get_codex_config_dir(),
        &crate::config::get_app_config_dir(),
        std::slice::from_ref(&account_token),
    )?;
    let client = reqwest::Client::builder()
        .user_agent(format!("ClawKit-Desktop/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(std::time::Duration::from_secs(8))
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(endpoint)
        .bearer_auth(&account_token)
        .header(reqwest::header::CONTENT_TYPE, "application/zip")
        .body(payload)
        .send()
        .await
        .map_err(|error| {
            format!(
                "上传诊断包失败：{}",
                crate::redact_known_secrets_strict(
                    &error.to_string(),
                    std::slice::from_ref(&account_token),
                )
            )
        })?;
    let status = response.status();
    let envelope = response
        .json::<UploadEnvelope>()
        .await
        .map_err(|_| "诊断服务响应无效".to_string())?;
    if !status.is_success() || envelope.code != 200 {
        return Err(if !envelope.message.trim().is_empty() {
            envelope.message
        } else if !envelope.detail.trim().is_empty() {
            envelope.detail
        } else {
            format!("诊断包上传失败 ({})", status.as_u16())
        });
    }
    let data = envelope
        .data
        .ok_or_else(|| "诊断服务响应缺少下载地址".to_string())?;
    if !data.url.starts_with("https://") {
        return Err("诊断服务返回了不安全的下载地址".to_string());
    }
    Ok(DiagnosticBundleUpload {
        bundle_id: data.bundle_id,
        url: data.url,
        expires_at: data.expires_at,
        expires_in_seconds: data.expires_in_seconds,
    })
}

fn build_bundle(
    codex_dir: &Path,
    app_dir: &Path,
    known_secrets: &[String],
) -> Result<Vec<u8>, String> {
    let mut secrets = known_secrets.to_vec();
    let config_path = codex_dir.join("config.toml");
    if let Ok(config) = std::fs::read_to_string(&config_path) {
        collect_toml_secrets(&config, &mut secrets);
    }

    let mut entries: Vec<(String, String)> = Vec::new();
    add_sanitized_file(
        &mut entries,
        "codex-config.toml",
        &config_path,
        false,
        &secrets,
    )?;
    add_sanitized_file(
        &mut entries,
        "clawkit-models.json",
        &codex_dir.join("clawkit-models.json"),
        false,
        &secrets,
    )?;
    add_sanitized_file(
        &mut entries,
        "cc-switch.log",
        &app_dir.join("logs").join("cc-switch.log"),
        true,
        &secrets,
    )?;
    add_sanitized_file(
        &mut entries,
        "crash.log",
        &app_dir.join("crash.log"),
        true,
        &secrets,
    )?;

    let included_files: Vec<&str> = entries.iter().map(|(name, _)| name.as_str()).collect();
    let manifest = serde_json::to_string_pretty(&json!({
        "schemaVersion": 1,
        "createdAt": chrono::Utc::now().to_rfc3339(),
        "appVersion": env!("CARGO_PKG_VERSION"),
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "includedFiles": included_files,
        "redacted": true,
    }))
    .map_err(|error| error.to_string())?;
    entries.insert(0, ("manifest.json".to_string(), manifest));
    entries.push((
        "README.txt".to_string(),
        "ClawKit diagnostic bundle\n\nThis archive is redacted before upload. It intentionally excludes auth.json, clawkit-account.json, databases, Codex sessions, request bodies, passwords, verification codes, cookies, and private keys. The download link is valid for 7 days and should only be shared with trusted support staff.\n".to_string(),
    ));

    let cursor = Cursor::new(Vec::new());
    let mut writer = zip::ZipWriter::new(cursor);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::default());
    for (name, content) in entries {
        writer
            .start_file(name, options)
            .map_err(|error| format!("创建诊断包失败：{error}"))?;
        writer
            .write_all(content.as_bytes())
            .map_err(|error| format!("写入诊断包失败：{error}"))?;
    }
    let payload = writer
        .finish()
        .map_err(|error| format!("完成诊断包失败：{error}"))?
        .into_inner();
    if payload.len() > MAX_BUNDLE_BYTES {
        return Err("诊断包超过 2 MiB，请先清理异常大的日志后重试".to_string());
    }
    Ok(payload)
}

fn add_sanitized_file(
    entries: &mut Vec<(String, String)>,
    archive_name: &str,
    path: &Path,
    tail: bool,
    known_secrets: &[String],
) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let content = if tail {
        read_tail(path, MAX_TEXT_BYTES)?
    } else {
        let bytes = std::fs::read(path)
            .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
        String::from_utf8_lossy(&bytes[..bytes.len().min(MAX_TEXT_BYTES)]).into_owned()
    };
    entries.push((
        archive_name.to_string(),
        redact_text(&content, known_secrets),
    ));
    Ok(())
}

fn read_tail(path: &Path, max_bytes: usize) -> Result<String, String> {
    let mut file =
        File::open(path).map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let length = file
        .metadata()
        .map_err(|error| format!("读取 {} 元信息失败：{error}", path.display()))?
        .len();
    let offset = length.saturating_sub(max_bytes as u64);
    file.seek(SeekFrom::Start(offset))
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("读取 {} 失败：{error}", path.display()))?;
    if offset > 0 {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn collect_toml_secrets(config: &str, output: &mut Vec<String>) {
    let Ok(value) = config.parse::<toml::Value>() else {
        return;
    };
    fn visit(value: &toml::Value, key: Option<&str>, output: &mut Vec<String>) {
        match value {
            toml::Value::String(text) if key.is_some_and(is_sensitive_key) && !text.is_empty() => {
                output.push(text.clone());
            }
            toml::Value::Array(values) => {
                for value in values {
                    visit(value, key, output);
                }
            }
            toml::Value::Table(table) => {
                for (key, value) in table {
                    visit(value, Some(key), output);
                }
            }
            _ => {}
        }
    }
    visit(&value, None, output);
}

fn is_sensitive_key(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    [
        "token",
        "secret",
        "password",
        "api_key",
        "apikey",
        "authorization",
        "cookie",
    ]
    .iter()
    .any(|marker| key.contains(marker))
}

fn redact_text(input: &str, known_secrets: &[String]) -> String {
    let exact = crate::redact_known_secrets_strict(input, known_secrets);
    let mut output = String::with_capacity(exact.len());
    for line in exact.lines() {
        let lower = line.to_ascii_lowercase();
        let assignment = line.contains('=') || line.contains(':');
        if assignment
            && [
                "authorization",
                "api_key",
                "apikey",
                "access_token",
                "refresh_token",
                "bearer_token",
                "experimental_bearer_token",
                "password",
                "secret",
                "cookie",
            ]
            .iter()
            .any(|marker| lower.contains(marker))
        {
            output.push_str("[REDACTED SENSITIVE LINE]\n");
        } else {
            output.push_str(line);
            output.push('\n');
        }
    }
    let output = BEARER_RE.replace_all(&output, "$1[REDACTED]");
    let output = API_KEY_RE.replace_all(&output, "[REDACTED]");
    JWT_RE.replace_all(&output, "[REDACTED]").into_owned()
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use super::{build_bundle, redact_text};

    #[test]
    fn bundle_contains_only_redacted_allowlisted_diagnostics() {
        let codex = tempfile::tempdir().expect("codex tempdir");
        let app = tempfile::tempdir().expect("app tempdir");
        std::fs::create_dir_all(app.path().join("logs")).unwrap();
        std::fs::write(
            codex.path().join("config.toml"),
            "model = \"gpt-test\"\nexperimental_bearer_token = \"opaque-secret-value\"\n",
        )
        .unwrap();
        std::fs::write(codex.path().join("auth.json"), "must-not-be-included").unwrap();
        std::fs::write(
            codex.path().join("clawkit-models.json"),
            "{\"models\":[\"gpt-test\"]}",
        )
        .unwrap();
        std::fs::write(
            app.path().join("logs/cc-switch.log"),
            "request Authorization: Bearer eyJabcdefgh.ijklmnop.qrstuvwx\napi sk-testsecret123\n",
        )
        .unwrap();

        let bytes = build_bundle(codex.path(), app.path(), &["session-secret".into()]).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = archive.file_names().map(str::to_string).collect();
        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&"codex-config.toml".to_string()));
        assert!(names.contains(&"cc-switch.log".to_string()));
        assert!(!names.contains(&"auth.json".to_string()));

        let mut combined = String::new();
        for index in 0..archive.len() {
            archive
                .by_index(index)
                .unwrap()
                .read_to_string(&mut combined)
                .unwrap();
        }
        assert!(!combined.contains("opaque-secret-value"));
        assert!(!combined.contains("sk-testsecret123"));
        assert!(!combined.contains("eyJabcdefgh.ijklmnop.qrstuvwx"));
    }

    #[test]
    fn redaction_hides_known_opaque_values_and_sensitive_assignments() {
        let redacted = redact_text(
            "endpoint ok\npassword = \"p@ss\"\nopaque session-secret value\n",
            &["session-secret".into()],
        );
        assert!(redacted.contains("endpoint ok"));
        assert!(!redacted.contains("p@ss"));
        assert!(!redacted.contains("session-secret"));
    }
}
