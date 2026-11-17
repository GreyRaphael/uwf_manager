//! 应用表单、状态枚举与辅助格式化类型。

use crate::backend::{ExclusionRevertPlan, FullSnapshot, RequestId};
use crate::model::FileExclusion;
use crate::ui::components::confirm::ConfirmState;
use crate::ui::components::input_dialog::InputState;

/// 写入缓存设置表单状态
#[derive(Debug, Default)]
pub struct CacheForm {
    pub max_size_text: String,
    pub warning_text: String,
    pub critical_text: String,
    /// 是否有未保存编辑；为 true 时自动刷新不覆盖用户输入
    pub dirty: bool,
    pub error: Option<String>,
}

impl CacheForm {
    pub fn sync_from_snapshot(&mut self, snapshot: &FullSnapshot) {
        self.max_size_text = snapshot
            .overlay_config_next
            .map(|c| c.max_size_mb.to_string())
            .unwrap_or_default();
        if let Some(rt) = snapshot.overlay_runtime {
            self.warning_text = rt.warning_threshold_mb.to_string();
            self.critical_text = rt.critical_threshold_mb.to_string();
        } else {
            self.warning_text.clear();
            self.critical_text.clear();
        }
        self.dirty = false;
        self.error = None;
    }

    pub fn on_edited(&mut self) {
        self.dirty = true;
        self.error = None;
    }

    pub fn sync_thresholds_from_runtime(&mut self, runtime: &crate::model::OverlayRuntime) {
        self.warning_text = runtime.warning_threshold_mb.to_string();
        self.critical_text = runtime.critical_threshold_mb.to_string();
        self.error = None;
    }

    pub fn validate(&self, snapshot: &FullSnapshot) -> Result<(u32, u32, u32), String> {
        let parse = |s: &str, what: &str| -> Result<u32, String> {
            s.trim()
                .parse::<u32>()
                .map_err(|_| format!("{what}须为 0-4294967295 的整数"))
        };
        let locked = is_overlay_locked(snapshot);
        let max_mb = if locked {
            snapshot
                .overlay_config_next
                .map(|c| c.max_size_mb)
                .ok_or_else(|| "配置尚未加载完成，无法保存".to_string())?
        } else {
            parse(&self.max_size_text, "最大容量")?
        };
        let warning_mb = parse(&self.warning_text, "警告阈值")?;
        let critical_mb = parse(&self.critical_text, "严重阈值")?;

        if max_mb == 0 {
            return Err("最大容量必须大于 0 MB".to_string());
        }
        if warning_mb >= critical_mb {
            return Err("警告阈值必须小于严重阈值".to_string());
        }
        if critical_mb > max_mb {
            return Err("严重阈值不能大于最大容量".to_string());
        }
        Ok((max_mb, warning_mb, critical_mb))
    }
}

/// R1：筛选器当前会话已启用时，缓存模式与最大容量锁定
pub fn is_overlay_locked(snapshot: &FullSnapshot) -> bool {
    snapshot.filter_state.is_some_and(|f| f.current_enabled)
}

/// 统一排除列表条目（文件/目录排除 或 注册表排除）
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExclusionItem {
    File(FileExclusion),
    Registry(String),
}

/// 缓存的单行排除项展示数据（避免每帧重算拼接与生效状态对比）
#[derive(Debug, Clone)]
pub struct ExclusionRow {
    pub item: ExclusionItem,
    pub display: String,
    pub in_current: bool,
}

impl ExclusionItem {
    pub fn display(&self) -> String {
        match self {
            Self::File(f) => f.display(),
            Self::Registry(k) => k.clone(),
        }
    }

    pub fn is_registry(&self) -> bool {
        matches!(self, Self::Registry(_))
    }
}

/// 当前打开的模态窗口
pub enum Modal {
    CommitMenu,
    Input(InputState),
    Confirm(ConfirmState),
    Help,
    Logs,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PowerAction {
    Restart,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MutationKind {
    General,
    /// 缓存表单保存成功时刷新表单，失败时把错误写入 `cache.error`
    CacheSave,
    /// 全局重置成功时强制刷新表单，失败时只记录全局/状态栏错误而不污染 `cache.error`
    CacheResetSync,
    Power(PowerAction),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InFlightMutation {
    pub id: RequestId,
    pub kind: MutationKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    Ok,
    Warn,
    Err,
}

#[derive(Debug, Clone)]
pub struct StatusEntry {
    pub time: String,
    pub level: LogLevel,
    pub text: String,
}

/// 应用生命周期阶段
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Loading,
    LoadFailed,
    NotInstalled,
    Ready,
    BackendLost,
}

pub(super) fn format_revert_confirm_message(plan: &ExclusionRevertPlan) -> String {
    let mut lines = vec![
        "将把「重启后生效」排除项列表恢复为与当前生效状态完全一致：".to_string(),
        String::new(),
    ];

    let remove_items: Vec<String> = plan
        .files_to_remove
        .iter()
        .map(FileExclusion::display)
        .chain(plan.regs_to_remove.iter().cloned())
        .collect();
    format_revert_section(&mut lines, "将移除（重启后新增）：", &remove_items, '-');

    let add_items: Vec<String> = plan
        .files_to_add
        .iter()
        .map(FileExclusion::display)
        .chain(plan.regs_to_add.iter().cloned())
        .collect();
    format_revert_section(&mut lines, "将恢复（当前仍生效但已删）：", &add_items, '+');

    lines.push(String::new());
    lines.push(format!(
        "预计移除 {} 项、恢复 {} 项。确定继续？",
        plan.remove_count(),
        plan.add_count(),
    ));
    lines.join("\n")
}

fn format_revert_section(lines: &mut Vec<String>, title: &str, items: &[String], prefix: char) {
    const MAX_PREVIEW: usize = 4;
    if items.is_empty() {
        return;
    }
    lines.push(title.to_string());
    for item in items.iter().take(MAX_PREVIEW) {
        lines.push(format!("  {prefix} {item}"));
    }
    if items.len() > MAX_PREVIEW {
        lines.push(format!("  … 等共 {} 项", items.len()));
    }
}
