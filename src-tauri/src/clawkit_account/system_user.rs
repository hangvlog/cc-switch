//! Remote device labels identify the signed-in OS user, never the host or installation ID.
pub(super) fn display_name() -> String {
    current_username().unwrap_or_else(|| "当前用户".to_owned())
}

fn normalize(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() || value.chars().any(char::is_control) {
        return None;
    }
    Some(value.chars().take(64).collect())
}

#[cfg(unix)]
fn current_username() -> Option<String> {
    use std::{ffi::CStr, mem::MaybeUninit, ptr};
    // Read the account database rather than environment variables inherited from a launcher.
    let mut entry = MaybeUninit::<libc::passwd>::uninit();
    let mut buffer = vec![0u8; 16 * 1024];
    let mut result = ptr::null_mut();
    let status = unsafe {
        libc::getpwuid_r(
            libc::geteuid(),
            entry.as_mut_ptr(),
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut result,
        )
    };
    if status != 0 || result.is_null() {
        return None;
    }
    let entry = unsafe { entry.assume_init() };
    if entry.pw_name.is_null() {
        return None;
    }
    normalize(unsafe { CStr::from_ptr(entry.pw_name) }.to_str().ok()?)
}

#[cfg(windows)]
fn current_username() -> Option<String> {
    // Standard Windows account API; UTF-16 preserves non-ASCII usernames.
    #[link(name = "advapi32")]
    extern "system" {
        fn GetUserNameW(buffer: *mut u16, size: *mut u32) -> i32;
    }
    let mut buffer = [0u16; 257];
    let mut size = buffer.len() as u32;
    if unsafe { GetUserNameW(buffer.as_mut_ptr(), &mut size) } == 0 || size < 2 {
        return None;
    }
    normalize(&String::from_utf16(&buffer[..(size as usize).checked_sub(1)?]).ok()?)
}

#[cfg(not(any(unix, windows)))]
fn current_username() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_use_account_names_without_machine_or_id_fallback() {
        assert_eq!(normalize(" hang \n"), Some("hang".into()));
        assert_eq!(normalize("张三"), Some("张三".into()));
        assert_eq!(normalize(" \n"), None);
        assert_eq!(normalize("bad\0name"), None);
        assert!(!display_name().is_empty());
    }
}
