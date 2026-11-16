use std::sync::mpsc::{Receiver, Sender};

use crate::error::{Result, UwfError};
use crate::model::*;

use super::logic::{extract_volume_guid_name, format_memory_gib};
use super::worker::{
    create_unique_export_file, export_exclusions, handle_command, import_exclusions,
    mutate_without_refresh,
};
use super::*;

#[test]
fn r6_split_absolute_path() {
    let (drive, rel) = split_absolute_path("C:\\Windows\\a.txt").unwrap();
    assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\Windows\\a.txt"));
    // 小写盘符归一化为大写
    let (drive, _) = split_absolute_path("d:\\data").unwrap();
    assert_eq!(drive, "D:");
    // C:foo 是 drive-relative，不是绝对路径，必须拒绝
    assert!(split_absolute_path("C:foo").is_err());
    // 正斜杠统一成反斜杠
    let (drive, rel) = split_absolute_path("C:/Windows/a").unwrap();
    assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\Windows\\a"));
    // 只有盘符时表示根目录
    let (drive, rel) = split_absolute_path("C:").unwrap();
    assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\"));
    // 非法盘符路径
    assert!(split_absolute_path("C::\\foo").is_err());
    // 相对路径报错
    assert!(matches!(
        split_absolute_path("Windows\\a.txt"),
        Err(UwfError::InvalidPath(_))
    ));
}

#[cfg(not(windows))]
#[test]
fn r6_unix_absolute_path_maps_to_demo_volume() {
    // 非 Windows 平台（Mock 预览）：Unix 绝对路径映射到演示卷 C:
    let (drive, rel) = split_absolute_path("/home/gewei/tmp").unwrap();
    assert_eq!((drive.as_str(), rel.as_str()), ("C:", "\\home\\gewei\\tmp"));
}

#[test]
fn r5_split_registry_path() {
    let (key, value) = split_registry_path("HKLM\\SOFTWARE\\X\\MyValue").unwrap();
    assert_eq!(
        (key.as_str(), value.as_str()),
        ("HKLM\\SOFTWARE\\X", "MyValue")
    );
    // 完全无反斜杠时报错；"HKLM\\SOFTWARE" 按规则可拆为 (HKLM, SOFTWARE)，合法
    assert!(matches!(
        split_registry_path("NOSPLIT"),
        Err(UwfError::InvalidPath(_))
    ));
    assert!(split_registry_path("").is_err());
}

#[test]
fn import_export_roundtrip_with_mock() {
    let dir = std::env::temp_dir().join(format!("uwf_mgr_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let import_path = dir.join("import.txt");
    std::fs::write(
        &import_path,
        "C:\\Windows\\Temp\nHKLM\\SOFTWARE\\Demo\nnot_a_path\n\nD:\\data\\cache\n",
    )
    .unwrap();

    let mut mock = MockUwfBackend::demo();
    let content = std::fs::read_to_string(&import_path).unwrap();
    let mut ok = 0;
    let mut failed = 0;
    for line in content.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let r = if line.starts_with("HK") {
            mock.add_registry_exclusion(line)
        } else {
            split_absolute_path(line).and_then(|(drive, rel)| mock.add_file_exclusion(&drive, &rel))
        };
        match r {
            Ok(()) => ok += 1,
            Err(_) => failed += 1,
        }
    }
    assert_eq!((ok, failed), (3, 1));
    // 全部写入 next 侧（R4），current 侧不受影响
    assert!(mock.get_file_exclusions(true).unwrap().is_empty());
    // 预置 1 条（与导入首行重复被去重）+ 实际新增 1 条 = 2
    assert_eq!(mock.get_file_exclusions(false).unwrap().len(), 2);
    // 预置 1 + 导入 1
    assert_eq!(mock.get_registry_exclusions(false).unwrap().len(), 2);

    let _ = std::fs::remove_dir_all(&dir);
}
fn test_sink() -> (Sender<BackendEvent>, Receiver<BackendEvent>) {
    std::sync::mpsc::channel()
}

#[test]
fn power_actions_finish_without_snapshot() {
    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    mutate_without_refresh(
        7,
        &mut backend,
        &|e| {
            let _ = tx.send(e);
        },
        |b| b.restart_system(),
        "系统即将重启",
    );
    match rx.try_recv().unwrap() {
        BackendEvent::Finished {
            id: 7,
            outcome: Outcome::Ok(msg),
        } => assert_eq!(msg, "系统即将重启"),
        other => panic!("期望仅收到 Finished(Ok)，实际 {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "电源动作不应再发 Snapshot");
}

#[test]
fn refresh_all_sends_snapshot() {
    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    handle_command(&mut backend, BackendCommand::RefreshAll, &|e| {
        let _ = tx.send(e);
    });
    match rx.try_recv().unwrap() {
        BackendEvent::Snapshot(snapshot) => {
            assert!(snapshot.filter_state.is_some());
        }
        other => panic!("期望 Snapshot，实际 {other:?}"),
    }
}
fn finished_outcomes(rx: &Receiver<BackendEvent>) -> Vec<Outcome> {
    let mut out = Vec::new();
    while let Ok(evt) = rx.try_recv() {
        if let BackendEvent::Finished { outcome, .. } = evt {
            out.push(outcome);
        }
    }
    out
}

#[test]
fn import_all_failed_is_error_partial_is_warning() {
    let dir = std::env::temp_dir().join(format!("uwf_import_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let all_fail = dir.join("all_fail.txt");
    std::fs::write(&all_fail, "not_a_path\n").unwrap();
    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    import_exclusions(1, &mut backend, &emit, &all_fail);
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Err(_)]
    ));

    let partial = dir.join("partial.txt");
    std::fs::write(&partial, "C:\\Windows\\Temp\nnot_a_path\n").unwrap();
    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    import_exclusions(2, &mut backend, &emit, &partial);
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Warning(_)]
    ));

    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn import_strips_utf8_bom_and_export_empty_is_zero_bytes() {
    let dir = std::env::temp_dir().join(format!("uwf_bom_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();

    let bom = dir.join("bom.txt");
    std::fs::write(&bom, "\u{feff}C:\\Windows\\Bom\n").unwrap();
    let mut backend = MockUwfBackend::demo();
    let (tx, _rx) = test_sink();
    import_exclusions(
        1,
        &mut backend,
        &|e| {
            let _ = tx.send(e);
        },
        &bom,
    );
    assert!(
        backend
            .get_file_exclusions(false)
            .unwrap()
            .iter()
            .any(|e| e.display() == "C:\\Windows\\Bom")
    );

    let export = dir.join("empty.txt");
    let mut empty = MockUwfBackend::empty();
    let (tx, _rx) = test_sink();
    export_exclusions(
        2,
        &mut empty,
        &|e| {
            let _ = tx.send(e);
        },
        &export,
    );
    assert_eq!(std::fs::metadata(&export).unwrap().len(), 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_rejects_non_utf8_with_clear_error() {
    let dir = std::env::temp_dir().join(format!("uwf_utf16_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("utf16.txt");
    std::fs::write(&path, [0xFF, 0xFE, b'C', 0, b':', 0]).unwrap();

    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    import_exclusions(
        1,
        &mut backend,
        &|e| {
            let _ = tx.send(e);
        },
        &path,
    );
    let outcomes = finished_outcomes(&rx);
    assert!(matches!(
        outcomes.as_slice(),
        [Outcome::Err(msg)] if msg.contains("UTF-8")
    ));

    let _ = std::fs::remove_dir_all(&dir);
}
#[test]
fn volume_union_includes_next_only() {
    let merged = merge_volume_infos(
        vec![("C:".to_string(), true)],
        vec![("c:".to_string(), false), ("D:".to_string(), true)],
    );
    assert_eq!(merged.len(), 2);
    assert!(
        merged
            .iter()
            .any(|v| v.drive_letter == "C:" && v.current_protected)
    );
    assert!(
        merged
            .iter()
            .any(|v| v.drive_letter == "D:" && !v.current_protected && v.next_protected)
    );
}

#[test]
fn unique_export_file_creates_non_colliding_files() {
    let dir = std::env::temp_dir().join(format!("uwf_export_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let base = dir.join("EXCLUSIONLIST_2026_10_20_12_00_00.txt");

    let (p1, _f1) = create_unique_export_file(&base).unwrap();
    let (p2, _f2) = create_unique_export_file(&base).unwrap();
    let (p3, _f3) = create_unique_export_file(&base).unwrap();
    assert_eq!(
        p1.file_name().and_then(|s| s.to_str()),
        Some("EXCLUSIONLIST_2026_10_20_12_00_00.txt")
    );
    assert_eq!(
        p2.file_name().and_then(|s| s.to_str()),
        Some("EXCLUSIONLIST_2026_10_20_12_00_00_1.txt")
    );
    assert_eq!(
        p3.file_name().and_then(|s| s.to_str()),
        Some("EXCLUSIONLIST_2026_10_20_12_00_00_2.txt")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn import_supports_lowercase_hk_and_reports_first_error() {
    let dir = std::env::temp_dir().join(format!("uwf_hk_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("hk.txt");
    // 首行小写 hklm，次行包含多字节汉字且非法路径，确认不 panic 且携带首个错误原因
    std::fs::write(&path, "hklm\\software\\demo\n中文非路径\n").unwrap();

    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    import_exclusions(
        3,
        &mut backend,
        &|e| {
            let _ = tx.send(e);
        },
        &path,
    );
    assert!(
        backend
            .get_registry_exclusions(false)
            .unwrap()
            .iter()
            .any(|k| k == "hklm\\software\\demo")
    );
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Warning(msg)] if msg.contains("首个错误") && msg.contains("中文非路径")
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

struct StubWarningOrNotInstalled {
    warnings: Vec<String>,
    not_installed: bool,
}

impl UwfBackend for StubWarningOrNotInstalled {
    fn get_filter_state(&mut self) -> Result<FilterState> {
        Err(UwfError::NotInstalled)
    }
    fn enable_filter(&mut self) -> Result<()> {
        Err(UwfError::NotInstalled)
    }
    fn disable_filter(&mut self) -> Result<()> {
        Ok(())
    }
    fn reset_settings(&mut self) -> Result<()> {
        Ok(())
    }
    fn restart_system(&mut self) -> Result<()> {
        Ok(())
    }
    fn shutdown_system(&mut self) -> Result<()> {
        Ok(())
    }
    fn get_overlay_config(&mut self, _: bool) -> Result<OverlayConfig> {
        Err(UwfError::NotInstalled)
    }
    fn get_overlay_runtime(&mut self) -> Result<OverlayRuntime> {
        Err(UwfError::NotInstalled)
    }
    fn set_overlay_type(&mut self, _: OverlayType) -> Result<()> {
        Ok(())
    }
    fn set_overlay_max_size(&mut self, _: u32) -> Result<()> {
        Ok(())
    }
    fn set_warning_threshold(&mut self, _: u32) -> Result<()> {
        Ok(())
    }
    fn set_critical_threshold(&mut self, _: u32) -> Result<()> {
        Ok(())
    }
    fn list_volumes(&mut self) -> Result<Vec<VolumeInfo>> {
        Ok(vec![])
    }
    fn protect_volume(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn unprotect_volume(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn commit_file(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    fn commit_file_deletion(&mut self, _: &str, _: &str) -> Result<()> {
        Err(UwfError::CommitDeleteAfterFileRemoved {
            path: "C:\\a".into(),
            message: "wmi err".into(),
        })
    }
    fn get_file_exclusions(&mut self, _: bool) -> Result<Vec<FileExclusion>> {
        if self.not_installed {
            Err(UwfError::NotInstalled)
        } else {
            Ok(vec![])
        }
    }
    fn add_file_exclusion(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    fn remove_file_exclusion(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    fn get_registry_exclusions(&mut self, _: bool) -> Result<Vec<String>> {
        Ok(vec![])
    }
    fn add_registry_exclusion(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn remove_registry_exclusion(&mut self, _: &str) -> Result<()> {
        Ok(())
    }
    fn commit_registry(&mut self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
    fn take_warnings(&mut self) -> Vec<String> {
        std::mem::take(&mut self.warnings)
    }
}

#[test]
fn export_drains_warnings_and_not_installed_propagates() {
    let dir = std::env::temp_dir().join(format!("uwf_warn_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let export = dir.join("out.txt");

    let mut stub = StubWarningOrNotInstalled {
        warnings: vec!["卷 D: 读取失败".into()],
        not_installed: false,
    };
    let (tx, rx) = test_sink();
    export_exclusions(
        1,
        &mut stub,
        &|e| {
            let _ = tx.send(e);
        },
        &export,
    );
    assert!(stub.warnings.is_empty(), "export 必须消费 warnings");
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Warning(msg)] if msg.contains("卷 D: 读取失败")
    ));

    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    handle_command(&mut stub, BackendCommand::RefreshAll, &emit);
    assert!(matches!(
        rx.try_recv().unwrap(),
        BackendEvent::NotInstalled(None)
    ));
    handle_command(
        &mut stub,
        BackendCommand::Mutate {
            id: 9,
            cmd: BackendCmd::EnableFilter,
        },
        &emit,
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        BackendEvent::NotInstalled(Some(9))
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn set_overlay_type_and_cache_respect_r1_lock() {
    let mut backend = MockUwfBackend::demo(); // current_enabled = true
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };

    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 1,
            cmd: BackendCmd::SetOverlayType(OverlayType::Disk),
        },
        &emit,
    );
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Err(msg)] if msg.contains("不可修改")
    ));

    // 当前启用时仅保存阈值，最大缓存保持 2048
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 2,
            cmd: BackendCmd::SetOverlayCache {
                max_mb: 9999,
                warning_mb: 1200,
                critical_mb: 1600,
            },
        },
        &emit,
    );
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Ok(msg)] if msg.contains("已锁定")
    ));
    assert_eq!(backend.get_overlay_config(false).unwrap().max_size_mb, 2048);
    let rt = backend.get_overlay_runtime().unwrap();
    assert_eq!(
        (rt.warning_threshold_mb, rt.critical_threshold_mb),
        (1200, 1600)
    );
}

fn raw_vol(
    session: bool,
    drive: Option<&str>,
    guid: Option<&str>,
    bind_by_drive: bool,
    protected: bool,
) -> RawVolumeInstance {
    let drive_label = drive.unwrap_or("none");
    let guid_label = guid.unwrap_or("empty");
    RawVolumeInstance {
        path: format!(
            "UWF_Volume.CurrentSession={session},DriveLetter=\"{drive_label}\",VolumeName=\"{guid_label}\""
        ),
        current_session: session,
        drive_letter: drive.map(str::to_string),
        volume_name: guid.map(str::to_string),
        bind_by_drive_letter: bind_by_drive,
        protected,
    }
}

#[test]
fn extract_volume_guid_name_normalizes_win32_device_id() {
    assert_eq!(
        extract_volume_guid_name(r"\\?\Volume{f0696b3f-f3ac-4e6e-8262-578ac0957ab8}\").as_deref(),
        Some("Volume{f0696b3f-f3ac-4e6e-8262-578ac0957ab8}")
    );
    assert_eq!(
        extract_volume_guid_name("  Volume{abc}  ").as_deref(),
        Some("Volume{abc}")
    );
    assert_eq!(extract_volume_guid_name(""), None);
    assert_eq!(extract_volume_guid_name(r"\\?\"), None);
}

#[test]
fn resolve_volumes_skips_missing_drive_and_avoids_mount_probe_without_conflict() {
    let rows = vec![
        raw_vol(true, None, Some("Volume{efi}"), false, false),
        raw_vol(true, Some(""), Some("Volume{recovery}"), false, false),
        raw_vol(true, Some("   "), Some("Volume{blank}"), false, false),
        raw_vol(true, Some("e:\\"), Some("Volume{15bed6ce}"), false, false),
        raw_vol(false, Some("c:"), Some("Volume{f0696b3f}"), true, true),
    ];
    let mut probed = Vec::new();
    let resolved = resolve_volumes(rows, |drive| {
        probed.push(drive.to_string());
        Ok(None)
    })
    .unwrap();

    assert!(probed.is_empty(), "无盘符冲突时不应触发挂载查询");
    assert_eq!(resolved.len(), 2);
    let e = find_session_volume(&resolved, true, "e:").unwrap();
    assert_eq!(e.drive_letter, "E:");
    assert_eq!(e.volume_name, "Volume{15bed6ce}");
    assert!(!e.bind_by_drive_letter);
    assert!(!e.protected);

    let c = find_session_volume(&resolved, false, "C:").unwrap();
    assert!(c.bind_by_drive_letter);
    assert!(c.protected);
    assert!(c.path.contains("CurrentSession=false"));
}

#[test]
fn resolve_volumes_disambiguates_duplicate_current_session_via_mount() {
    let rows = vec![
        raw_vol(true, Some("E:"), Some("Volume{15bed6ce}"), false, false),
        raw_vol(true, Some("H:"), Some("Volume{df5bd4a8}"), false, false),
        raw_vol(true, Some("H:"), Some("Volume{4ff2d2fc}"), false, false),
        raw_vol(false, Some("C:"), Some("Volume{f0696b3f}"), false, true),
    ];
    let mut probed = Vec::new();
    let resolved = resolve_volumes(rows, |drive| {
        probed.push(drive.to_string());
        Ok(match drive {
            "H:" => Some(r"\\?\Volume{4FF2D2FC}\".to_string()),
            _ => None,
        })
    })
    .unwrap();

    assert_eq!(probed, vec!["H:"]);
    let h = find_session_volume(&resolved, true, "H:").unwrap();
    assert_eq!(h.volume_name, "Volume{4ff2d2fc}");
    assert!(find_session_volume(&resolved, true, "E:").is_some());
    assert!(find_session_volume(&resolved, false, "C:").is_some());
}

#[test]
fn resolve_volumes_drops_unmounted_ghosts_and_handles_next_session_conflict() {
    // 全离线冲突行应被安全滤除而不报错
    let ghost_rows = vec![
        raw_vol(true, Some("H:"), Some("Volume{old-1}"), false, false),
        raw_vol(true, Some("H:"), Some("Volume{old-2}"), false, false),
    ];
    let resolved_ghosts = resolve_volumes(ghost_rows, |_| Ok(None)).unwrap();
    assert!(resolved_ghosts.is_empty());

    // 下次会话中 BindByDriveLetter = true 的配置即使旧 GUID 已卸载也应保留
    let next_conflict = vec![
        raw_vol(false, Some("D:"), Some("Volume{old-usb}"), true, true),
        raw_vol(false, Some("D:"), Some("Volume{stale}"), false, false),
    ];
    let resolved_next = resolve_volumes(next_conflict, |_| Ok(None)).unwrap();
    assert_eq!(resolved_next.len(), 1);
    assert_eq!(resolved_next[0].volume_name, "Volume{old-usb}");
    assert!(resolved_next[0].protected);

    // 下次会话同时存在“按盘符绑定”和“按 GUID 绑定且已挂载”两个不同实例时必须报歧义
    let true_ambiguity = vec![
        raw_vol(false, Some("D:"), Some("Volume{by-letter}"), true, true),
        raw_vol(
            false,
            Some("D:"),
            Some("Volume{mounted-guid}"),
            false,
            false,
        ),
    ];
    let err = resolve_volumes(true_ambiguity, |_| {
        Ok(Some(r"\\?\Volume{mounted-guid}\".to_string()))
    })
    .unwrap_err();
    assert!(matches!(err, UwfError::VolumeAmbiguous(ref d) if d == "D:"));

    // 同会话同盘符同 GUID 重复出现同样报错
    let dup_identity = vec![
        raw_vol(true, Some("E:"), Some("Volume{same}"), false, false),
        raw_vol(true, Some("E:"), Some("VOLUME{SAME}"), false, false),
    ];
    assert!(matches!(
        resolve_volumes(dup_identity, |_| Ok(None)),
        Err(UwfError::VolumeAmbiguous(ref d)) if d == "E:"
    ));
}

#[test]
fn skips_current_unprotected_volumes_when_querying_file_exclusions() {
    assert!(should_query_volume_exclusions(true, true));
    assert!(!should_query_volume_exclusions(true, false));
    assert!(should_query_volume_exclusions(false, true));
    assert!(should_query_volume_exclusions(false, false));
}

#[test]
fn commit_actions_require_enabled_filter_and_current_protected_volume() {
    // 1. 仅保护 C: 时，对未保护的 D: 执行 CommitFile / CommitFileDeletion 必须直接被拒绝
    let mut backend = MockUwfBackend::demo();
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 1,
            cmd: BackendCmd::CommitFile("D:\\test.txt".into()),
        },
        &emit,
    );
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 2,
            cmd: BackendCmd::CommitFileDeletion("D:\\test.txt".into()),
        },
        &emit,
    );
    // C: 受保护且筛选器已启用，应允许提交
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 3,
            cmd: BackendCmd::CommitFileDeletion("C:\\data\\ok.txt".into()),
        },
        &emit,
    );
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [
            Outcome::Err(e1),
            Outcome::Err(e2),
            Outcome::Ok(ok3),
        ] if e1.contains("卷 D: 当前会话未受 UWF 保护")
            && e2.contains("卷 D: 当前会话未受 UWF 保护")
            && ok3.contains("删除提交成功")
    ));

    // 2. 停用筛选器并重启后，即使之前保护过 C:，现在也必须拦截所有提交/固化操作
    backend.disable_filter().unwrap();
    backend.restart_system().unwrap();
    let (tx, rx) = test_sink();
    let emit = |e| {
        let _ = tx.send(e);
    };
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 4,
            cmd: BackendCmd::CommitFile("C:\\data\\ok.txt".into()),
        },
        &emit,
    );
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 5,
            cmd: BackendCmd::CommitFileDeletion("C:\\data\\ok.txt".into()),
        },
        &emit,
    );
    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 6,
            cmd: BackendCmd::CommitRegistry("HKLM\\SOFTWARE\\Demo\\Val".into()),
        },
        &emit,
    );
    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Err(e1), Outcome::Err(e2), Outcome::Err(e3)]
            if e1.contains("筛选器当前会话未启用")
                && e2.contains("筛选器当前会话未启用")
                && e3.contains("筛选器当前会话未启用")
    ));
}

#[test]
fn os_version_formatting_and_edition_detection() {
    let gib = 1024_u64 * 1024 * 1024;
    let formatted = format_os_version(
        "Microsoft Windows 10 Enterprise LTSC",
        Some("19044"),
        Some(16 * gib),
    );
    assert_eq!(formatted, "Windows 10 Enterprise LTSC 2021 16G");
    assert_eq!(
        format_os_version(
            "Microsoft Windows 11 Enterprise",
            Some("22631"),
            // 核显保留后 TotalPhysicalMemory 略低于 32 GiB 的常见数值应四舍五入到 32G
            Some(31 * gib + 800 * 1024 * 1024),
        ),
        "Windows 11 Enterprise 32G"
    );
    assert_eq!(
        format_os_version("Microsoft Windows 10 教育版", None, None),
        "Windows 10 教育版"
    );
    assert_eq!(format_memory_gib(0), None);
    assert_eq!(format_memory_gib(512 * 1024 * 1024), Some("1G".into()));
    assert_eq!(format_memory_gib(256 * 1024 * 1024), Some("256M".into()));

    assert_eq!(
        check_windows_edition_support(&formatted),
        EditionSupport::Supported
    );
    assert_eq!(
        check_windows_edition_support("Windows 11 教育版 16G"),
        EditionSupport::Supported
    );
    assert_eq!(
        check_windows_edition_support("Windows 11 Pro 16G"),
        EditionSupport::Unsupported
    );
    assert_eq!(check_windows_edition_support(""), EditionSupport::Unknown);
}

#[test]
fn revert_exclusions_to_current_restores_exact_current_set() {
    let mut backend = MockUwfBackend::demo();
    // 先“重启”让演示规则生效为当前基线：current == next == [Windows\Temp, UwfManagerDemo]
    backend.restart_system().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let emit = |evt: BackendEvent| {
        let _ = tx.send(evt);
    };

    // 触发不对称差异：新增 1 项文件与 1 项注册表，同时移除基线中的 1 项文件与 1 项注册表
    backend.add_file_exclusion("C:", "\\Extra\\Temp").unwrap();
    backend
        .remove_file_exclusion("C:", "\\Windows\\Temp")
        .unwrap();
    backend
        .add_registry_exclusion("HKLM\\SOFTWARE\\Extra")
        .unwrap();
    backend
        .remove_registry_exclusion("HKLM\\SOFTWARE\\UwfManagerDemo")
        .unwrap();

    let plan = plan_exclusion_revert(
        &backend.get_file_exclusions(true).unwrap(),
        &backend.get_file_exclusions(false).unwrap(),
        &backend.get_registry_exclusions(true).unwrap(),
        &backend.get_registry_exclusions(false).unwrap(),
    );
    assert_eq!(plan.remove_count(), 2);
    assert_eq!(plan.add_count(), 2);

    handle_command(
        &mut backend,
        BackendCommand::Mutate {
            id: 99,
            cmd: BackendCmd::RevertExclusionsToCurrent,
        },
        &emit,
    );

    assert!(matches!(
        finished_outcomes(&rx).as_slice(),
        [Outcome::Ok(msg)] if msg.contains("移除 2 项") && msg.contains("恢复 2 项")
    ));
    assert_eq!(
        backend.get_file_exclusions(false).unwrap(),
        backend.get_file_exclusions(true).unwrap()
    );
    assert_eq!(
        backend.get_registry_exclusions(false).unwrap(),
        backend.get_registry_exclusions(true).unwrap()
    );
}
