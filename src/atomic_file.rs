use std::path::Path;

use anyhow::{bail, Context, Result};

pub fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    if !destination.exists() {
        return std::fs::rename(temporary, destination)
            .with_context(|| format!("ファイルを確定できません: {}", destination.display()));
    }
    replace_existing_file(temporary, destination)
}

#[cfg(unix)]
fn replace_existing_file(temporary: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(temporary, destination).with_context(|| {
        format!(
            "ファイルを原子的に置換できません: {}",
            destination.display()
        )
    })
}

#[cfg(windows)]
fn replace_existing_file(temporary: &Path, destination: &Path) -> Result<()> {
    use std::{ffi::OsStr, iter, os::windows::ffi::OsStrExt};

    fn wide(path: &Path) -> Vec<u16> {
        OsStr::new(path)
            .encode_wide()
            .chain(iter::once(0))
            .collect()
    }

    let destination = wide(destination);
    let temporary = wide(temporary);
    // SAFETY: どちらのパスもNUL終端のUTF-16文字列で、呼び出し中は有効です。
    let replaced = unsafe {
        ReplaceFileW(
            destination.as_ptr(),
            temporary.as_ptr(),
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    };
    if replaced == 0 {
        // SAFETY: ReplaceFileW直後にスレッドローカルのエラーコードを取得します。
        let error = unsafe { GetLastError() };
        bail!("ファイルを原子的に置換できません (Windows エラー {error})");
    }
    Ok(())
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn ReplaceFileW(
        replaced_file_name: *const u16,
        replacement_file_name: *const u16,
        backup_file_name: *const u16,
        replace_flags: u32,
        exclude: *mut std::ffi::c_void,
        reserved: *mut std::ffi::c_void,
    ) -> i32;
    fn GetLastError() -> u32;
}

#[cfg(not(any(unix, windows)))]
fn replace_existing_file(temporary: &Path, destination: &Path) -> Result<()> {
    std::fs::rename(temporary, destination)
        .with_context(|| format!("ファイルを置換できません: {}", destination.display()))
}

#[cfg(unix)]
pub fn sync_directory(directory: &Path) -> Result<()> {
    std::fs::File::open(directory)
        .and_then(|file| file.sync_all())
        .with_context(|| format!("ディレクトリを同期できません: {}", directory.display()))
}

#[cfg(not(unix))]
pub fn sync_directory(_directory: &Path) -> Result<()> {
    Ok(())
}
