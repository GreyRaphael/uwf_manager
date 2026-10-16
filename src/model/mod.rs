//! 业务数据模型：与 WMI `root\standardcimv2\embedded` 各类一一对应。

mod exclusion;
mod filter;
mod overlay;
mod volume;

pub use exclusion::FileExclusion;
pub use filter::FilterState;
pub use overlay::{OverlayConfig, OverlayRuntime, OverlayType, UsageLevel};
pub use volume::VolumeInfo;
