//! 全部中文文案常量（D7：预留 i18n 文本层，不实现双语切换）。

use crate::backend::BackendKind;

pub const READY: &str = "就绪";
pub const LOADING: &str = "加载中…";
pub const READY_EMPTY: &str = "（无）";
pub const HOTKEY_BAR: &str = "[Tab] 切换页面  [↑/↓] 选择  [←/→] 切换栏  [F5] 全部刷新  [Q] 退出";

pub fn app_title() -> String {
    format!(" UWF Manager v{} ", env!("CARGO_PKG_VERSION"))
}

pub fn backend_badge(kind: BackendKind) -> &'static str {
    match kind {
        BackendKind::Windows => "[已获管理员权限]",
        BackendKind::Mock => "[Mock 演示数据]",
    }
}

pub fn protected_zh(value: bool) -> &'static str {
    if value { "受保护" } else { "未保护" }
}

// ── D6：UWF 未安装引导页 ──
pub const NI_TAB_LABEL: &str = "UWF 功能未安装 — 引导页";
pub const NI_TITLE: &str = "[UWF 功能未安装]";
pub const NI_LEAD: &str = "未检测到 Windows 统一写入筛选器（UWF），管理功能暂不可用。";
pub const NI_REASON_TITLE: &str = "可能原因：";
pub const NI_REASON_1: &str =
    "1. 系统未启用 UWF 可选功能（仅 Windows 企业版 / 教育版 / IoT 提供）";
pub const NI_REASON_2: &str = "2. WMI 命名空间 root\\standardcimv2\\embedded 不可用";
pub const NI_ENABLE_TITLE: &str = "启用方法（管理员 PowerShell 执行，重启后生效）：";
pub const NI_ENABLE_CMD: &str =
    "Enable-WindowsOptionalFeature -Online -FeatureName Client-UnifiedWriteFilter";
pub const NI_FOOTER: &str = "完成启用并重启后按 [F5] 重新检测；[Q] 退出。";

pub fn enabled_zh(value: bool) -> &'static str {
    if value { "启用" } else { "禁用" }
}
