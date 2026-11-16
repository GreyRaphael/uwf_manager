//! 后端抽象层：UI 与底层 UWF 实现的隔离边界。
//!
//! 线程协议（PLAN v2 §6.5）：UI 线程发送 [`BackendCommand`]（查询、带 [`RequestId`] 的变更或退出），
//! 后端线程独占 `dyn UwfBackend` 执行后经 [`BackendEvent`] 回包。变更类命令
//! 成功后自动跟随一次全量刷新，刷新结果通过 [`Outcome`] 与动作结果一起上报。

mod logic;
mod mock;
mod protocol;
#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;
mod worker;

pub use logic::{
    EditionSupport, ExclusionRevertPlan, check_windows_edition_support, plan_exclusion_revert,
    split_absolute_path, split_registry_path, validate_filter_commit_ready,
    validate_volume_commit_ready,
};
#[cfg(any(windows, test))]
pub(crate) use logic::{
    RawVolumeInstance, find_session_volume, format_os_version, merge_volume_infos, resolve_volumes,
    should_query_volume_exclusions,
};
#[cfg(windows)]
pub(crate) use logic::{ResolvedVolume, normalize_drive_letter};
pub use mock::MockUwfBackend;
pub use protocol::{
    BackendCmd, BackendCommand, BackendEvent, BackendKind, FullSnapshot, Outcome, RequestId,
    UwfBackend, create_backend_factory,
};
#[cfg(windows)]
pub(crate) use windows::is_elevated;
pub use worker::spawn_backend_thread_with_wake;
