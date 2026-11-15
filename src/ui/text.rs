//! 界面中文文案与共享标签常量。

use crate::backend::BackendKind;
pub use crate::error::UWF_ENABLE_CMD as NI_ENABLE_CMD;

pub const READY: &str = "就绪";
pub const LOADING: &str = "加载中…";
pub const LOAD_FAILED: &str = "加载失败，按 [F5] 重试";
pub const READY_EMPTY: &str = "（无）";

pub const NOTICE_BAR: &str =
    "注意: 覆盖类型与最大缓存需禁用写入过滤并重启后更改；设置需重启生效 (仅企业/教育版支持 UWF)";
pub const NOTICE_BAR_SHORT: &str =
    "注意: 覆盖类型/最大缓存需禁用写入过滤并重启后可改；设置需重启生效";
pub const HOTKEY_BAR: &str =
    "[Tab] 切区  [↑/↓] 选择  [Enter/空格/点击] 操作  [C] 提交固化  [F5] 刷新  [?] 帮助  [Q] 退出";
pub const HOTKEY_BAR_SHORT: &str =
    "[Tab]切区 [↑/↓]选 [Enter/点击]操作 [C]提交 [F5]刷新 [?]帮助 [Q]退出";
pub const NI_HOTKEY_BAR: &str = "[F5] 重新检测  [?] 帮助  [Q] 退出";

pub fn app_title() -> String {
    format!("UWF管理器 v{}", env!("CARGO_PKG_VERSION"))
}

pub fn backend_badge(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Windows => "[管理员]",
        BackendKind::Mock => "[Mock演示]",
    }
}

pub fn protected_zh(value: bool) -> &'static str {
    if value { "已保护" } else { "不保护" }
}

pub fn enabled_zh(value: bool) -> &'static str {
    if value { "已启用" } else { "已禁用" }
}

pub fn toggle_zh(value: bool) -> &'static str {
    if value { "启用" } else { "禁用" }
}

// ── D6：UWF 未安装引导页 ──
pub const NI_TITLE: &str = " UWF 功能未安装 ";
pub const NI_LEAD: &str = "未检测到 Windows 统一写入筛选器（UWF），管理功能暂不可用。";
pub const NI_REASON_TITLE: &str = "可能原因：";
pub const NI_REASON_1: &str = "1. 系统未启用 UWF 可选功能（仅 Windows 企业版 / 教育版 / IoT 提供）";
pub const NI_REASON_2: &str = "2. WMI 命名空间 root\\standardcimv2\\embedded 不可用";
pub const NI_ENABLE_TITLE: &str = "启用方法（管理员 PowerShell 执行，重启后生效）：";
pub const NI_FOOTER: &str = "完成启用并重启后按 [F5] 重新检测；[Q] 退出。";
