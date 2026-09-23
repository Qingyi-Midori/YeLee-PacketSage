//! Windows 凭据管理器里的 provider 密钥（《GUI 工程规格书 v0.2》§7.4 第 3 步 / U7）。
//!
//! 界面层的一条硬规矩：**api key 不落明文文件**。`%LOCALAPPDATA%\PacketSage\`
//! 下的 `provider.json` 只放 provider / model / base_url，key 走
//! `CredWriteW` 存进凭据管理器（generic credential，owner-only，随用户漫游关闭）。

/// 凭据的 target 名；一个应用一条，用户名写 provider 名便于诊断。
pub const TARGET: &str = "PacketSage:llm-api-key";

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    /// UTF-16 + NUL（Win32 的 `PCWSTR` 就是它）。
    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn last_error(what: &str) -> String {
        // SAFETY: `GetLastError` 只读线程本地状态。
        let code = unsafe { GetLastError() };
        format!("{what} failed with Win32 error {code}")
    }

    /// 写入（或覆盖）密钥；用户名写 `user`（一般是 provider 名）。
    pub fn write(target_name: &str, user: &str, secret: &str) -> Result<(), String> {
        let mut target = wide(target_name);
        let mut username = wide(user);
        let mut blob = secret.as_bytes().to_vec();
        let size = u32::try_from(blob.len()).map_err(|_| "secret is too long".to_owned())?;
        let credential = CREDENTIALW {
            Flags: 0,
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: std::ptr::null_mut(),
            LastWritten: Default::default(),
            CredentialBlobSize: size,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            AttributeCount: 0,
            Attributes: std::ptr::null_mut(),
            TargetAlias: std::ptr::null_mut(),
            UserName: username.as_mut_ptr(),
        };
        // SAFETY: 三个缓冲区（target / username / blob）都活到调用结束，
        // 结构体按 Win32 定义填齐，`CredWriteW` 只读它们。
        let ok = unsafe { CredWriteW(&credential, 0) };
        if ok != 0 {
            return Ok(());
        }
        Err(last_error("CredWriteW"))
    }

    /// 读回密钥；没有这条凭据时返回 `Ok(None)`。
    pub fn read(target_name: &str) -> Result<Option<String>, String> {
        let target = wide(target_name);
        let mut raw: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: `CredReadW` 成功时把一块由 CredFree 释放的内存写进 `raw`。
        let ok = unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut raw) };
        if ok == 0 {
            // SAFETY: 只读线程本地错误码。
            let code = unsafe { GetLastError() };
            if code == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(last_error("CredReadW"));
        }
        // SAFETY: `ok != 0` 保证 `raw` 指向一块有效的 CREDENTIALW。
        let (size, pointer) = unsafe { ((*raw).CredentialBlobSize, (*raw).CredentialBlob) };
        let bytes: &[u8] = if pointer.is_null() || size == 0 {
            &[]
        } else {
            // SAFETY: 凭据管理器保证这块内存有 `size` 字节。
            unsafe { std::slice::from_raw_parts(pointer, size as usize) }
        };
        let secret = String::from_utf8(bytes.to_vec()).map_err(|error| error.to_string());
        // SAFETY: `raw` 来自 CredReadW，必须用 CredFree 还。
        unsafe { CredFree(raw as *const c_void) };
        secret.map(Some)
    }

    /// 删除密钥；本来就没有时返回 `Ok(false)`。
    pub fn delete(target_name: &str) -> Result<bool, String> {
        let target = wide(target_name);
        // SAFETY: 只传一个以 NUL 结尾的宽字符串。
        let ok = unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) };
        if ok != 0 {
            return Ok(true);
        }
        // SAFETY: 只读线程本地错误码。
        let code = unsafe { GetLastError() };
        if code == ERROR_NOT_FOUND {
            return Ok(false);
        }
        Err(last_error("CredDeleteW"))
    }
}

#[cfg(not(windows))]
mod stub {
    //! 非 Windows 只有编译价值：桌面外壳本身只在 Windows 上交付（§7.7）。

    pub fn write(_target: &str, _user: &str, _secret: &str) -> Result<(), String> {
        Err("凭据管理器只在 Windows 上可用".to_owned())
    }

    pub fn read(_target: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    pub fn delete(_target: &str) -> Result<bool, String> {
        Ok(false)
    }
}

#[cfg(windows)]
use imp as backend;
#[cfg(not(windows))]
use stub as backend;

/// 写入应用那条凭据。
pub fn write(user: &str, secret: &str) -> Result<(), String> {
    backend::write(TARGET, user, secret)
}

/// 读回应用那条凭据。
pub fn read() -> Result<Option<String>, String> {
    backend::read(TARGET)
}

/// 删除应用那条凭据（返回"本来有吗"）。
pub fn delete() -> Result<bool, String> {
    backend::delete(TARGET)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 真写一条凭据再删掉：CredMan 的 FFI 很容易"看着对、实际读不回来"。
    /// 用**独立 target**，绝不碰用户那条 `PacketSage:llm-api-key`。
    #[test]
    fn secret_round_trips_through_the_credential_manager() {
        let probe = format!("PacketSage:test-{}", std::process::id());
        let target = format!("{probe}:target");
        backend::write(&target, "probe", &probe).expect("write");
        let read_back = backend::read(&target).expect("read").expect("a secret");
        assert_eq!(read_back, probe);
        assert!(backend::delete(&target).expect("delete"));
        assert_eq!(backend::read(&target).expect("read"), None);
        assert!(!backend::delete(&target).expect("delete again"));
    }
}
