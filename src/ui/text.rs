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

pub fn enabled_zh(value: bool) -> &'static str {
    if value { "启用" } else { "禁用" }
}
