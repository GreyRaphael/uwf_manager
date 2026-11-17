//! Windows 版本与物理内存只读查询（Win32 Registry + Memory API）。
//!
//! 直接读取 `HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion` 与 SMBIOS/系统物理内存，
//! 避免启动期额外建立 `ROOT\CIMV2` WMI COM 连接与 WMI 查询开销。
#![allow(unsafe_code)]

use windows::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows::Win32::System::Registry::{HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW};
use windows::Win32::System::SystemInformation::{
    GetPhysicallyInstalledSystemMemory, GlobalMemoryStatusEx, MEMORYSTATUSEX,
};
use windows::core::{PCWSTR, w};

const CURRENT_VERSION_SUBKEY: PCWSTR = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");

/// 查询当前 Windows 版本与物理内存规格，格式化为如 `Windows 10 Enterprise LTSC 2021 16G`。
pub(super) fn query_os_version() -> Option<String> {
    let product_name = read_current_version_sz(w!("ProductName"))?;
    let build = read_current_version_sz(w!("CurrentBuildNumber"))
        .or_else(|| read_current_version_sz(w!("CurrentBuild")));
    let memory_bytes = query_total_memory_bytes();
    Some(crate::backend::format_os_version(
        &product_name,
        build.as_deref(),
        memory_bytes,
    ))
}

fn read_current_version_sz(value_name: PCWSTR) -> Option<String> {
    let mut cb_data: u32 = 0;
    // SAFETY: `CURRENT_VERSION_SUBKEY` 与 `value_name` 为静态 NUL 结尾宽字符串；
    // 首次调用仅传入 `cb_data` 指针查询缓冲字节长度。
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CURRENT_VERSION_SUBKEY,
            value_name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut cb_data),
        )
    };
    if (status != ERROR_SUCCESS && status != ERROR_MORE_DATA) || cb_data == 0 {
        return None;
    }

    let mut buf = vec![0_u16; (cb_data as usize).div_ceil(2)];
    let mut byte_len = u32::try_from(buf.len().saturating_mul(2)).ok()?;
    // SAFETY: `buf` 按 `byte_len` 对齐分配并独占可写；`RRF_RT_REG_SZ` 仅返回 NUL 结尾宽字符数据。
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            CURRENT_VERSION_SUBKEY,
            value_name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut byte_len),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }

    let u16_len = ((byte_len as usize) / 2).min(buf.len());
    let slice = buf[..u16_len].split(|&ch| ch == 0).next()?;
    let text = String::from_utf16_lossy(slice).trim().to_string();
    (!text.is_empty()).then_some(text)
}

/// 优先读取 SMBIOS 安装内存（保留核显占用前的标称容量），不可用时回退到 `GlobalMemoryStatusEx`。
fn query_total_memory_bytes() -> Option<u64> {
    let mut total_kb: u64 = 0;
    // SAFETY: `total_kb` 为有效的 `u64` 栈变量指针；API 只写 KiB 总量。
    if unsafe { GetPhysicallyInstalledSystemMemory(&mut total_kb) }.is_ok() && total_kb > 0 {
        return Some(total_kb.saturating_mul(1024));
    }

    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?,
        ..Default::default()
    };
    // SAFETY: `status.dwLength` 已设为其精确字节大小，由 Win32 只写填充字段。
    if unsafe { GlobalMemoryStatusEx(&mut status) }.is_ok() && status.ullTotalPhys > 0 {
        return Some(status.ullTotalPhys);
    }
    None
}
