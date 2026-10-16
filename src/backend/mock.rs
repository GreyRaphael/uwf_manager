//! Mock 后端：全平台仿真 UWF 数据，用于 Linux/macOS 开发预览与单元测试。
//!
//! 完整实现"下次会话"状态机：enable/protect 等只改 next 侧，
//! `restart_system` 将 next 快照应用为 current（模拟重启生效）。

use crate::error::{Result, UwfError};
use crate::model::*;

use super::UwfBackend;

struct MockVolume {
    letter: String,
    current: bool,
    next: bool,
}

pub struct MockUwfBackend {
    filter_current: bool,
    filter_next: bool,
    overlay_type_current: OverlayType,
    overlay_type_next: OverlayType,
    max_size_current: u32,
    max_size_next: u32,
    warning_mb: u32,
    critical_mb: u32,
    consumption_mb: u32,
    volumes: Vec<MockVolume>,
    file_excl_current: Vec<FileExclusion>,
    file_excl_next: Vec<FileExclusion>,
    reg_excl_current: Vec<String>,
    reg_excl_next: Vec<String>,
}

impl MockUwfBackend {
    /// 演示数据：C: 受保护、D: 未保护；筛选器当前启用（可演示 R1 锁定）；
    /// RAM 型 Overlay 2048 MB。
    pub fn demo() -> Self {
        Self {
            filter_current: true,
            filter_next: true,
            overlay_type_current: OverlayType::Ram,
            overlay_type_next: OverlayType::Ram,
            max_size_current: 2048,
            max_size_next: 2048,
            warning_mb: 1536,
            critical_mb: 1843,
            consumption_mb: 512,
            volumes: vec![
                MockVolume { letter: "C:".into(), current: true, next: true },
                MockVolume { letter: "D:".into(), current: false, next: false },
            ],
            file_excl_current: vec![],
            file_excl_next: vec![FileExclusion::new("C:".into(), "\\Windows\\Temp".into())],
            reg_excl_current: vec![],
            reg_excl_next: vec!["HKLM\\SOFTWARE\\UwfManagerDemo".into()],
        }
    }

    fn find_volume_mut(&mut self, drive_letter: &str) -> Result<&mut MockVolume> {
        let upper = drive_letter.to_ascii_uppercase();
        self.volumes
            .iter_mut()
            .find(|v| v.letter == upper)
            .ok_or_else(|| UwfError::VolumeNotFound(drive_letter.into()))
    }

    fn known_volume(&self, drive_letter: &str) -> Result<()> {
        let upper = drive_letter.to_ascii_uppercase();
        if self.volumes.iter().any(|v| v.letter == upper) {
            Ok(())
        } else {
            Err(UwfError::VolumeNotFound(drive_letter.into()))
        }
    }
}

impl UwfBackend for MockUwfBackend {
    fn get_filter_state(&mut self) -> Result<FilterState> {
        Ok(FilterState {
            current_enabled: self.filter_current,
            next_enabled: self.filter_next,
        })
    }

    fn enable_filter(&mut self) -> Result<()> {
        self.filter_next = true;
        Ok(())
    }

    fn disable_filter(&mut self) -> Result<()> {
        self.filter_next = false;
        Ok(())
    }

    fn reset_settings(&mut self) -> Result<()> {
        // 模拟 ResetSettings：下次启动恢复出厂配置（禁用、RAM 1024、无排除）
        self.filter_next = false;
        self.overlay_type_next = OverlayType::Ram;
        self.max_size_next = 1024;
        self.warning_mb = 768;
        self.critical_mb = 921;
        for v in &mut self.volumes {
            v.next = false;
        }
        self.file_excl_next.clear();
        self.reg_excl_next.clear();
        Ok(())
    }

    fn restart_system(&mut self) -> Result<()> {
        // 模拟重启：next 快照应用为 current
        self.filter_current = self.filter_next;
        self.overlay_type_current = self.overlay_type_next;
        self.max_size_current = self.max_size_next;
        for v in &mut self.volumes {
            v.current = v.next;
        }
        self.file_excl_current = self.file_excl_next.clone();
        self.reg_excl_current = self.reg_excl_next.clone();
        Ok(())
    }

    fn shutdown_system(&mut self) -> Result<()> {
        Ok(())
    }

    fn get_overlay_config(&mut self, current_session: bool) -> Result<OverlayConfig> {
        Ok(if current_session {
            OverlayConfig {
                current_session: true,
                overlay_type: self.overlay_type_current,
                max_size_mb: self.max_size_current,
            }
        } else {
            OverlayConfig {
                current_session: false,
                overlay_type: self.overlay_type_next,
                max_size_mb: self.max_size_next,
            }
        })
    }

    fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime> {
        // 每次调用小幅增长，演示 Gauge 颜色梯度（绿→黄→红）
        self.consumption_mb = (self.consumption_mb + 7).min(self.max_size_current);
        Ok(OverlayRuntime {
            available_space_mb: self.max_size_current.saturating_sub(self.consumption_mb),
            consumption_mb: self.consumption_mb,
            warning_threshold_mb: self.warning_mb,
            critical_threshold_mb: self.critical_mb,
        })
    }

    fn set_overlay_type(&mut self, overlay_type: OverlayType) -> Result<()> {
        // R1：筛选器当前启用时锁定
        if self.filter_current {
            return Err(UwfError::FilterCurrentlyEnabled { what: "Overlay 类型" });
        }
        self.overlay_type_next = overlay_type;
        Ok(())
    }

    fn set_overlay_max_size(&mut self, size_mb: u32) -> Result<()> {
        if self.filter_current {
            return Err(UwfError::FilterCurrentlyEnabled { what: "Overlay 最大容量" });
        }
        self.max_size_next = size_mb;
        Ok(())
    }

    fn set_warning_threshold(&mut self, size_mb: u32) -> Result<()> {
        self.warning_mb = size_mb;
        Ok(())
    }

    fn set_critical_threshold(&mut self, size_mb: u32) -> Result<()> {
        self.critical_mb = size_mb;
        Ok(())
    }

    fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>> {
        Ok(self
            .volumes
            .iter()
            .map(|v| VolumeInfo {
                drive_letter: v.letter.clone(),
                current_protected: v.current,
                next_protected: v.next,
            })
            .collect())
    }

    fn protect_volume(&mut self, drive_letter: &str) -> Result<()> {
        // R2：只改下次会话侧
        self.find_volume_mut(drive_letter)?.next = true;
        Ok(())
    }

    fn unprotect_volume(&mut self, drive_letter: &str) -> Result<()> {
        self.find_volume_mut(drive_letter)?.next = false;
        Ok(())
    }

    fn commit_file(&mut self, drive_letter: &str, _file_rel_path: &str) -> Result<()> {
        self.known_volume(drive_letter)
    }

    fn commit_file_deletion(&mut self, drive_letter: &str, _file_rel_path: &str) -> Result<()> {
        // R3 第一步（删除真实文件）在 Mock 下无意义，直接模拟成功
        self.known_volume(drive_letter)
    }

    fn get_file_exclusions(&mut self, current_session: bool) -> Result<Vec<FileExclusion>> {
        Ok(if current_session {
            self.file_excl_current.clone()
        } else {
            self.file_excl_next.clone()
        })
    }

    fn add_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        // R4：只改下次会话侧
        self.known_volume(drive_letter)?;
        let item = FileExclusion::new(drive_letter.to_ascii_uppercase(), file_rel_path.into());
        if !self.file_excl_next.contains(&item) {
            self.file_excl_next.push(item);
            self.file_excl_next.sort();
        }
        Ok(())
    }

    fn remove_file_exclusion(&mut self, drive_letter: &str, file_rel_path: &str) -> Result<()> {
        let item = FileExclusion::new(drive_letter.to_ascii_uppercase(), file_rel_path.into());
        self.file_excl_next.retain(|x| x != &item);
        Ok(())
    }

    fn get_registry_exclusions(&mut self, current_session: bool) -> Result<Vec<String>> {
        Ok(if current_session {
            self.reg_excl_current.clone()
        } else {
            self.reg_excl_next.clone()
        })
    }

    fn add_registry_exclusion(&mut self, key: &str) -> Result<()> {
        let key = key.to_string();
        if !self.reg_excl_next.contains(&key) {
            self.reg_excl_next.push(key);
            self.reg_excl_next.sort();
        }
        Ok(())
    }

    fn remove_registry_exclusion(&mut self, key: &str) -> Result<()> {
        self.reg_excl_next.retain(|x| x != key);
        Ok(())
    }

    fn commit_registry(&mut self, _key: &str, _value_name: &str) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r1_overlay_locked_when_filter_currently_enabled() {
        let mut b = MockUwfBackend::demo(); // demo 默认当前启用
        assert!(matches!(
            b.set_overlay_type(OverlayType::Disk),
            Err(UwfError::FilterCurrentlyEnabled { .. })
        ));
        assert!(matches!(
            b.set_overlay_max_size(4096),
            Err(UwfError::FilterCurrentlyEnabled { .. })
        ));
        // 阈值不受 R1 限制
        assert!(b.set_warning_threshold(1000).is_ok());
        assert!(b.set_critical_threshold(1800).is_ok());
    }

    #[test]
    fn r1_unlocked_after_disable_and_restart() {
        let mut b = MockUwfBackend::demo();
        b.disable_filter().unwrap();
        b.restart_system().unwrap(); // 模拟重启后 current=disabled
        b.set_overlay_type(OverlayType::Disk).unwrap();
        b.set_overlay_max_size(4096).unwrap();
        let next = b.get_overlay_config(false).unwrap();
        assert_eq!(next.overlay_type, OverlayType::Disk);
        assert_eq!(next.max_size_mb, 4096);
    }

    #[test]
    fn r2_protect_only_changes_next_session() {
        let mut b = MockUwfBackend::demo();
        b.protect_volume("D:").unwrap();
        let volumes = b.list_volumes().unwrap();
        let d = volumes.iter().find(|v| v.drive_letter == "D:").unwrap();
        assert!(!d.current_protected);
        assert!(d.next_protected);
        // 重启后生效
        b.restart_system().unwrap();
        let volumes = b.list_volumes().unwrap();
        let d = volumes.iter().find(|v| v.drive_letter == "D:").unwrap();
        assert!(d.current_protected);
        // 未知卷报错
        assert!(matches!(
            b.protect_volume("Z:"),
            Err(UwfError::VolumeNotFound(_))
        ));
    }

    #[test]
    fn r4_exclusion_mutation_only_changes_next_session() {
        let mut b = MockUwfBackend::demo();
        b.add_file_exclusion("D:", "\\data\\cache").unwrap();
        assert!(b.get_file_exclusions(true).unwrap().is_empty());
        let next = b.get_file_exclusions(false).unwrap();
        assert!(next.iter().any(|e| e.display() == "D:\\data\\cache"));
        // 去重
        b.add_file_exclusion("D:", "\\data\\cache").unwrap();
        assert_eq!(b.get_file_exclusions(false).unwrap().len(), 2);
        // 注册表排除同理
        b.add_registry_exclusion("HKLM\\SOFTWARE\\X").unwrap();
        assert!(b.get_registry_exclusions(true).unwrap().is_empty());
        b.restart_system().unwrap();
        // demo 预置 1 条 + 新增 1 条，重启后全部生效
        assert_eq!(b.get_registry_exclusions(true).unwrap().len(), 2);
    }

    #[test]
    fn overlay_consumption_grows_and_caps_at_max() {
        let mut b = MockUwfBackend::demo();
        let first = b.get_overlay_runtime().unwrap().consumption_mb;
        assert!(first > 512);
        for _ in 0..500 {
            b.get_overlay_runtime().unwrap();
        }
        let rt = b.get_overlay_runtime().unwrap();
        assert_eq!(rt.consumption_mb, 2048);
        assert_eq!(rt.available_space_mb, 0);
        assert_eq!(rt.usage_level(), UsageLevel::Critical);
        assert_eq!(rt.usage_percent(2048), 100);
    }
}
