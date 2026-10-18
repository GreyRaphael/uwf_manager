//! 管理员提权检测（PLAN D5 兜底）。
//!
//! crate 级 `#![deny(unsafe_code)]` 下唯一豁免的 Win32 边界：
//! `OpenProcessToken` + `GetTokenInformation` 均为只读查询，无内存副作用。
#![allow(unsafe_code)]

use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

/// 当前进程是否已提权（TokenElevation）。
/// 检测失败按未提权处理（manifest 缺失时引导用户提权，而非放行）。
pub fn is_elevated() -> bool {
    unsafe {
        let mut token = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).is_err() {
            return false;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut return_length = std::mem::size_of::<TOKEN_ELEVATION>() as u32;
        let result = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            return_length,
            &mut return_length,
        );
        let _ = CloseHandle(token);
        result.is_ok() && elevation.TokenIsElevated != 0
    }
}
