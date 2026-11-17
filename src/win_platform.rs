//! Windows 桌面运行期内存优化辅助：
//! 1. 绕开 `DirectWrite` 全系统字体扫描与多套字体并发回退，直接 `mmap` 映射最少系统字体；
//! 2. 直接绑定 PE `.rc` 资源图标（ID 2），避免启动期解码并常驻 256×256 RGBA 位图与 PNG 重编码；
//! 3. 启动稳定及最小化时提示系统修剪冷页工作集。
//!
//! 本模块仅在 `cfg(windows)` 下编译，集中存放必要的系统调用 `unsafe` 边界。

#![allow(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;
use eframe::egui::epaint::text::{Blob, FontData, FontDefinitions, FontFamily};
use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::{GetCurrentProcess, SetProcessWorkingSetSize};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, ICON_BIG, ICON_SMALL, IMAGE_ICON, LR_DEFAULTCOLOR, LR_SHARED, LoadImageW,
    SM_CXICON, SM_CXSMICON, SM_CYICON, SM_CYSMICON, SendMessageW, WM_SETICON,
};
use windows::core::PCWSTR;

/// `uwf_manager.rc` 中嵌入的应用图标资源编号（`2 ICON "icon.ico"`）。
const PE_ICON_RESOURCE_ID: usize = 2;

// 优先使用 Microsoft YaHei UI（msyh.ttc 第 1 号字面，行高更紧凑贴合 Windows UI），
// 若字库仅含单字面则由 ttc_preferred_index 自动回退到 0。
const CJK_CANDIDATES: &[(&str, u32)] = &[
    ("msyh.ttc", 1),
    ("simhei.ttf", 0),
    ("simsun.ttc", 0),
    ("segoeui.ttf", 0),
];

/// 仅内存映射单一系统 UI 字体（优先微软雅黑 UI），等宽与正文字族复用同一字面并配合矢量图标，
/// 避免额外挂载 Consolas / Segoe UI Symbol 及 `DirectWrite` 全量枚举带来的私有内存占用。
pub fn configure_fonts(ctx: &egui::Context) {
    let fonts_dir = windows_fonts_dir();
    let mut fonts = FontDefinitions::empty();

    if !insert_first_font(&mut fonts, "win_cjk", &fonts_dir, CJK_CANDIDATES) {
        return;
    }

    for family in [FontFamily::Proportional, FontFamily::Monospace] {
        if let Some(list) = fonts.families.get_mut(&family) {
            list.push("win_cjk".to_owned());
        }
    }

    ctx.set_fonts(fonts);
}

/// 从 PE 资源直接加载多尺寸 `.ico` 并设置窗口大/小图标（与 `IconData::default()` 配合跳过 `eframe` 图标解码/重编码）。
pub fn apply_pe_window_icon(cc: &eframe::CreationContext<'_>) {
    let Ok(handle) = cc.window_handle() else {
        return;
    };
    let RawWindowHandle::Win32(win32) = handle.as_raw() else {
        return;
    };
    let hwnd = HWND(std::ptr::without_provenance_mut(win32.hwnd.get() as usize));
    let res_id = PCWSTR::from_raw(std::ptr::without_provenance::<u16>(PE_ICON_RESOURCE_ID));

    // SAFETY: GetModuleHandleW(None) 返回当前进程 exe 模块句柄；LoadImageW 使用 LR_SHARED 由系统管理句柄生命周期，
    // SendMessageW(WM_SETICON) 仅向本线程刚创建的窗口发送共享图标句柄。
    unsafe {
        let Ok(hmod) = GetModuleHandleW(None) else {
            return;
        };
        let hinst = HINSTANCE::from(hmod);

        for (icon_slot, cx_metric, cy_metric) in [
            (ICON_BIG, SM_CXICON, SM_CYICON),
            (ICON_SMALL, SM_CXSMICON, SM_CYSMICON),
        ] {
            let cx = GetSystemMetrics(cx_metric);
            let cy = GetSystemMetrics(cy_metric);
            if let Ok(icon) = LoadImageW(
                Some(hinst),
                res_id,
                IMAGE_ICON,
                cx,
                cy,
                LR_DEFAULTCOLOR | LR_SHARED,
            ) {
                let _ = SendMessageW(
                    hwnd,
                    WM_SETICON,
                    Some(WPARAM(icon_slot as usize)),
                    Some(LPARAM(icon.0 as isize)),
                );
            }
        }
    }
}

/// 提示 Windows 内存管理器修剪冷页工作集（不限制进程最大内存，仅在启动稳定及最小化时按需调用）。
pub fn trim_working_set() {
    // SAFETY: GetCurrentProcess() 为伪句柄无需关闭；传 usize::MAX 即 (SIZE_T)-1 指示内核修剪闲置页面。
    unsafe {
        let _ = SetProcessWorkingSetSize(GetCurrentProcess(), usize::MAX, usize::MAX);
    }
}

fn windows_fonts_dir() -> PathBuf {
    std::env::var_os("WINDIR")
        .or_else(|| std::env::var_os("SystemRoot"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
        .join("Fonts")
}

fn ttc_preferred_index(bytes: &[u8], preferred: u32) -> u32 {
    if preferred == 0 || bytes.len() < 12 || &bytes[..4] != b"ttcf" {
        return 0;
    }
    let num_fonts = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if preferred < num_fonts { preferred } else { 0 }
}

fn insert_first_font(
    fonts: &mut FontDefinitions,
    key: &str,
    fonts_dir: &Path,
    candidates: &[(&str, u32)],
) -> bool {
    for &(file_name, preferred_index) in candidates {
        let path = fonts_dir.join(file_name);
        if let Some(mapped) = mmap_font_file(&path) {
            let face_index = ttc_preferred_index(&mapped, preferred_index);
            let blob: Blob = Arc::new(mapped);
            fonts.font_data.insert(
                key.to_owned(),
                Arc::new(FontData::from_blob(blob, face_index)),
            );
            return true;
        }
    }
    false
}

fn mmap_font_file(path: &Path) -> Option<memmap2::Mmap> {
    let file = std::fs::File::open(path).ok()?;
    // SAFETY: Windows Fonts 目录下的系统字体在只读映射期内不会发生外部截断；仅以只读共享切片形式提供给字体解析器。
    unsafe { memmap2::Mmap::map(&file).ok() }
}
