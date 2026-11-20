use crate::model::TranslationProvider;
use windows::core::{HSTRING, HRESULT, PWSTR};
use windows::Win32::Foundation::ERROR_NOT_FOUND;
use windows::Win32::Security::Credentials::{
    CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
    CRED_TYPE_GENERIC,
};

fn target(provider: TranslationProvider) -> HSTRING {
    HSTRING::from(format!("app.spotify-lyrics-translator.desktop/{}", provider.account()))
}

pub fn read_key(provider: TranslationProvider) -> Result<Option<String>, String> {
    let name = target(provider);
    let mut credential = std::ptr::null_mut();
    // A successful CredReadW owns one allocation, released after copying the blob.
    unsafe {
        match CredReadW(&name, CRED_TYPE_GENERIC, None, &mut credential) {
            Ok(()) => {
                let entry = &*credential;
                let result = if entry.CredentialBlobSize == 0 {
                    Ok(None)
                } else {
                    let bytes = std::slice::from_raw_parts(
                        entry.CredentialBlob,
                        entry.CredentialBlobSize as usize,
                    );
                    std::str::from_utf8(bytes)
                        .map(|key| Some(key.to_owned()))
                        .map_err(|_| "Windows 認證管理員的憑證格式無效".into())
                };
                CredFree(credential.cast());
                result
            }
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(None),
            Err(_) => Err("無法讀取 Windows 認證管理員".into()),
        }
    }
}

pub fn write_key(provider: TranslationProvider, key: &str) -> Result<(), String> {
    let name = target(provider);
    if key.is_empty() {
        // The target is null-terminated and remains alive throughout the call.
        return match unsafe { CredDeleteW(&name, CRED_TYPE_GENERIC, None) } {
            Ok(()) => Ok(()),
            Err(error) if error.code() == HRESULT::from_win32(ERROR_NOT_FOUND.0) => Ok(()),
            Err(_) => Err("無法移除 Windows 認證管理員的憑證".into()),
        };
    }
    let mut bytes = key.as_bytes().to_vec();
    let credential = CREDENTIALW {
        Type: CRED_TYPE_GENERIC,
        TargetName: PWSTR(name.as_ptr().cast_mut()),
        CredentialBlobSize: u32::try_from(bytes.len()).map_err(|_| "API key 過長")?,
        CredentialBlob: bytes.as_mut_ptr(),
        Persist: CRED_PERSIST_LOCAL_MACHINE,
        ..Default::default()
    };
    // CredWriteW copies the credential while the name and byte buffer are alive.
    unsafe { CredWriteW(&credential, 0) }.map_err(|_| "無法更新 Windows 認證管理員".into())
}
