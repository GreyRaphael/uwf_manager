use super::*;
use crate::model::{FileExclusion, FilterState, OverlayConfig, OverlayRuntime, VolumeInfo};
use egui_kittest::{Harness, kittest::Queryable as _};
use std::cell::RefCell;
use std::rc::Rc;

#[derive(Clone, Default)]
struct StubFileDialog {
    replies: Rc<RefCell<VecDeque<Option<PathBuf>>>>,
}

impl StubFileDialog {
    fn with_replies(items: impl IntoIterator<Item = Option<PathBuf>>) -> Self {
        Self {
            replies: Rc::new(RefCell::new(items.into_iter().collect())),
        }
    }
}

impl FileDialogService for StubFileDialog {
    fn open(&mut self, _kind: FileDialogKind, ctx: &egui::Context) -> Receiver<Option<PathBuf>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let reply = self.replies.borrow_mut().pop_front().flatten();
        let _ = tx.send(reply);
        ctx.request_repaint();
        rx
    }
}

pub(crate) fn demo_snapshot() -> FullSnapshot {
    FullSnapshot {
        filter_state: Some(FilterState {
            current_enabled: true,
            next_enabled: true,
        }),
        overlay_config_current: Some(OverlayConfig {
            overlay_type: OverlayType::Ram,
            max_size_mb: 2048,
        }),
        overlay_config_next: Some(OverlayConfig {
            overlay_type: OverlayType::Ram,
            max_size_mb: 2048,
        }),
        overlay_runtime: Some(OverlayRuntime {
            consumption_mb: 512,
            warning_threshold_mb: 1536,
            critical_threshold_mb: 1843,
        }),
        volumes: vec![
            VolumeInfo {
                drive_letter: "C:".into(),
                current_protected: true,
                next_protected: true,
            },
            VolumeInfo {
                drive_letter: "D:".into(),
                current_protected: false,
                next_protected: false,
            },
        ],
        file_exclusions_current: vec![],
        file_exclusions_next: vec![FileExclusion::new("C:".into(), "\\Windows\\Temp".into())],
        registry_exclusions_current: vec![],
        registry_exclusions_next: vec!["HKLM\\SOFTWARE\\UwfManagerDemo".into()],
        os_version: Some("Windows 10 Enterprise LTSC 2021 16G".into()),
        warnings: vec![],
    }
}

fn test_app(kind: BackendKind) -> (UwfApp, Receiver<BackendCommand>) {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (_, event_rx) = std::sync::mpsc::channel();
    (
        UwfApp::new_with_file_dialog(cmd_tx, event_rx, kind, Box::new(StubFileDialog::default())),
        cmd_rx,
    )
}

fn ready_app(kind: BackendKind) -> (UwfApp, Receiver<BackendCommand>) {
    let (mut app, cmd_rx) = test_app(kind);
    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    (app, cmd_rx)
}

fn recv_cmd(rx: &Receiver<BackendCommand>) -> BackendCmd {
    match rx.try_recv().expect("期望收到后端命令") {
        BackendCommand::Mutate { cmd, .. } => cmd,
        other => panic!("期望 Mutate 命令，实际 {other:?}"),
    }
}

fn build_gui_harness<'a>(
    kind: BackendKind,
    dialog: StubFileDialog,
    snapshot: Option<FullSnapshot>,
) -> (
    Harness<'a, UwfApp>,
    Sender<BackendEvent>,
    Receiver<BackendCommand>,
) {
    let (cmd_tx, cmd_rx) = std::sync::mpsc::channel();
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    if let Some(snap) = snapshot {
        let _ = event_tx.send(BackendEvent::Snapshot(snap));
    }
    let harness = Harness::builder()
        .with_size([1240.0, 780.0])
        .allow_missing_glyphs()
        .build_eframe(move |cc| {
            theme::apply_theme(&cc.egui_ctx);
            UwfApp::new_with_file_dialog(cmd_tx, event_rx, kind, Box::new(dialog))
        });
    (harness, event_tx, cmd_rx)
}

#[test]
fn not_installed_event_shows_guide_and_snapshot_recovers() {
    let (mut app, _cmd_rx) = test_app(BackendKind::Mock);
    assert!(app.is_busy());
    app.apply_event(BackendEvent::NotInstalled(None));
    assert_eq!(app.phase, Phase::NotInstalled);
    assert!(!app.is_busy());
    assert!(app.last_log().is_some_and(|e| e.level == LogLevel::Err));
    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    assert_eq!(app.phase, Phase::Ready);
}

#[test]
fn initial_load_error_and_retry_failure_transition_cleanly() {
    let (mut app, cmd_rx) = test_app(BackendKind::Mock);
    app.apply_event(BackendEvent::RefreshFailed("temporary WMI failure".into()));
    assert!(!app.is_busy());
    assert_eq!(app.phase, Phase::LoadFailed);

    app.refresh_all();
    assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));
    assert_eq!(app.phase, Phase::Loading);
    app.apply_event(BackendEvent::RefreshFailed("second failure".into()));
    assert_eq!(app.phase, Phase::LoadFailed);

    app.refresh_all();
    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    assert_eq!(app.phase, Phase::Ready);
    assert!(!app.is_busy());
    app.refresh_all();
    app.apply_event(BackendEvent::RefreshFailed("transient".into()));
    assert_eq!(app.phase, Phase::Ready);
}

#[test]
fn query_events_do_not_clear_mutation() {
    let (mut app, _cmd_rx) = ready_app(BackendKind::Mock);
    let mutation = app.begin_mutation(BackendCmd::EnableFilter).unwrap();

    app.apply_event(BackendEvent::RefreshFailed("刷新失败".into()));
    assert_eq!(app.mutation, Some(mutation));

    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    assert_eq!(app.mutation, Some(mutation));

    app.apply_event(BackendEvent::NotInstalled(None));
    assert_eq!(app.mutation, Some(mutation));

    app.apply_event(BackendEvent::NotInstalled(Some(mutation)));
    assert_eq!(app.mutation, None);

    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    let mutation = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
    app.apply_event(BackendEvent::Finished {
        id: mutation,
        outcome: Outcome::Ok("ok".into()),
    });
    assert_eq!(app.mutation, None);
}

#[test]
fn mutation_gated_while_pending() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.toggle_write_filter();
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::DisableFilter));
    assert_eq!(app.mutation, Some(id));

    app.set_volume_protected("C:", false);
    assert!(cmd_rx.try_recv().is_err());

    app.apply_event(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("ok".into()),
    });
    assert!(!app.is_pending());
    app.set_volume_protected("C:", false);
    assert!(matches!(recv_cmd(&cmd_rx), BackendCmd::Unprotect(_)));
}

#[test]
fn power_action_not_armed_when_mutation_is_pending() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    let first = app.begin_mutation(BackendCmd::EnableFilter).unwrap();
    let _ = cmd_rx.try_recv().unwrap();

    app.execute_confirm(ConfirmAction::Restart);
    assert!(cmd_rx.try_recv().is_err());
    assert!(app.power_action.is_none());

    app.apply_event(BackendEvent::Finished {
        id: first,
        outcome: Outcome::Ok("UWF 将在下次系统重启后启用".into()),
    });
    assert!(!app.should_quit);
}

#[test]
fn not_installed_and_backend_lost_skip_overlay_poll() {
    let (mut app, cmd_rx) = test_app(BackendKind::Mock);
    app.send_overlay_refresh();
    assert!(cmd_rx.try_recv().is_err());

    app.apply_event(BackendEvent::NotInstalled(None));
    app.send_overlay_refresh();
    assert!(cmd_rx.try_recv().is_err());

    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    app.send_overlay_refresh();
    assert!(matches!(
        cmd_rx.try_recv(),
        Ok(BackendCommand::RefreshOverlayRuntime)
    ));

    app.apply_event(BackendEvent::Disconnected);
    app.send_overlay_refresh();
    assert!(cmd_rx.try_recv().is_err());
}

#[test]
fn basic_settings_toggles_write_filter_and_respects_r1_lock() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.toggle_write_filter();
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate");
    };
    assert!(matches!(cmd, BackendCmd::DisableFilter));
    app.apply_event(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("ok".into()),
    });

    // current_enabled == true 时 R1 锁定切换缓存模式
    app.set_overlay_type(OverlayType::Disk);
    assert!(cmd_rx.try_recv().is_err());

    // 解锁后允许切换缓存模式
    let mut snap = demo_snapshot();
    snap.filter_state = Some(FilterState {
        current_enabled: false,
        next_enabled: false,
    });
    app.apply_event(BackendEvent::Snapshot(snap));
    app.set_overlay_type(OverlayType::Disk);
    assert!(matches!(
        recv_cmd(&cmd_rx),
        BackendCmd::SetOverlayType(OverlayType::Disk)
    ));
}

#[test]
fn cache_form_validation_dirty_flag_and_reset() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.cache.warning_text = "2000".into();
    app.cache.on_edited();
    app.submit_cache_settings();
    assert!(cmd_rx.try_recv().is_err());
    assert!(
        app.cache
            .error
            .as_deref()
            .is_some_and(|e| e.contains("必须小于"))
    );

    // dirty = true 时后台刷新不冲掉正在编辑的文本
    app.apply_event(BackendEvent::Snapshot(demo_snapshot()));
    assert_eq!(app.cache.warning_text, "2000");

    // 还原恢复到快照值
    app.reset_cache_form();
    assert!(!app.cache.dirty);
    assert_eq!(app.cache.warning_text, "1536");

    // 合法修改后提交
    app.cache.warning_text = "1530".into();
    app.cache.on_edited();
    app.submit_cache_settings();
    match recv_cmd(&cmd_rx) {
        BackendCmd::SetOverlayCache {
            max_mb,
            warning_mb,
            critical_mb,
        } => {
            assert_eq!((max_mb, warning_mb, critical_mb), (2048, 1530, 1843));
        }
        other => panic!("期望 SetOverlayCache，实际 {other:?}"),
    }
}

#[test]
fn reset_settings_error_does_not_pollute_cache_error_and_success_syncs_form() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.cache.warning_text = "1400".into();
    app.cache.on_edited();
    app.execute_confirm(ConfirmAction::ResetSettings);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::ResetSettings));

    // 重置失败不应写进缓存表单的 cache.error，只记全局错误日志
    app.apply_event(BackendEvent::Finished {
        id,
        outcome: Outcome::Err("WMI ResetSettings 失败".into()),
    });
    assert!(app.cache.error.is_none());
    assert!(app.cache.dirty);
    assert!(
        app.last_log()
            .is_some_and(|e| e.level == LogLevel::Err && e.text.contains("ResetSettings"))
    );

    // 重置成功时应强制把缓存表单同步为新快照值并清除 dirty
    app.execute_confirm(ConfirmAction::ResetSettings);
    let BackendCommand::Mutate { id: id2, .. } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    let mut reset_snap = demo_snapshot();
    reset_snap.overlay_runtime = Some(OverlayRuntime {
        consumption_mb: 0,
        warning_threshold_mb: 768,
        critical_threshold_mb: 921,
    });
    app.apply_event(BackendEvent::Snapshot(reset_snap));
    app.apply_event(BackendEvent::Finished {
        id: id2,
        outcome: Outcome::Ok("已重置".into()),
    });
    assert!(!app.cache.dirty);
    assert_eq!(app.cache.warning_text, "768");
    assert!(app.cache.error.is_none());
}

#[test]
fn gui_delete_key_does_not_remove_exclusion_and_confirm_modal_ignores_enter() {
    let (mut harness, _event_tx, cmd_rx) = build_gui_harness(
        BackendKind::Mock,
        StubFileDialog::default(),
        Some(demo_snapshot()),
    );

    // 主界面按 Delete 不应未确认直删任何排除项
    harness.key_press(egui::Key::Delete);
    harness.run_steps(2);
    assert!(cmd_rx.try_recv().is_err());

    // 高危确认弹窗按回车不得自动触发确认动作（防止焦点停在“取消”或误按回车）
    harness.get_by_label("立即重启生效…").click();
    harness.run();
    assert!(harness.query_by_label("重启系统确认").is_some());
    harness.key_press(egui::Key::Enter);
    harness.run_steps(2);
    assert!(cmd_rx.try_recv().is_err());
    assert!(harness.query_by_label("重启系统确认").is_some());
}

#[test]
fn restart_flow_mock_stays_windows_quits() {
    let (mut app, cmd_rx) = ready_app(BackendKind::Mock);
    app.execute_confirm(ConfirmAction::Restart);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::RestartSystem));
    app.apply_event(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("系统即将重启".into()),
    });
    assert!(!app.should_quit);
    assert!(
        app.last_log()
            .is_some_and(|e| e.text.contains("（模拟）已重启"))
    );

    let (mut app, cmd_rx) = ready_app(BackendKind::Windows);
    app.execute_confirm(ConfirmAction::Restart);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::RestartSystem));
    app.apply_event(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("系统即将重启".into()),
    });
    assert!(app.should_quit);
}

#[test]
fn gui_dashboard_renders_and_dispatches_filter_volume_and_cache_actions() {
    let (mut harness, event_tx, cmd_rx) = build_gui_harness(
        BackendKind::Mock,
        StubFileDialog::default(),
        Some(demo_snapshot()),
    );

    // 顶栏已去除重复窗口标题、筛选器当前状态标签与刷新按钮
    assert!(harness.query_by_label("刷新 (F5)").is_none());
    assert!(harness.query_by_label_contains("UWF 管理器").is_none());
    assert!(harness.query_by_label_contains("当前: ").is_none());

    // 切换写保护总开关（执行期间顶部 Spinner 持续重绘，使用 run_steps 推进一帧）
    harness.get_by_label("切换写保护总开关").click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::DisableFilter));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("ok".into()),
    });
    harness.run();

    // 受保护分区已改为每行直接开关切换，顶部不再存在「保护选中分区 / 取消保护」按钮
    assert!(harness.query_by_label("保护选中分区").is_none());
    assert!(harness.query_by_label("取消保护").is_none());

    // 初始 2 个分区的「当前状态 / 重启后状态」分别为 (保护, 保护) 与 (不保护, 不保护)，仅 2 条排除项显示「待重启生效」
    assert_eq!(harness.query_all_by_label("保护").count(), 2);
    assert_eq!(harness.query_all_by_label("不保护").count(), 2);
    assert_eq!(harness.query_all_by_label("待重启生效").count(), 2);

    // 直接拨动 D: 行开关开启保护（不保护 -> on(待重启生效) -> 保护）
    harness.get_by_label("切换 D: 重启后保护").click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Protect 命令");
    };
    assert!(matches!(cmd, BackendCmd::Protect(ref drive) if drive == "D:"));

    let mut updated = demo_snapshot();
    updated.volumes[1].next_protected = true;
    let _ = event_tx.send(BackendEvent::Snapshot(updated.clone()));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("D: 保护成功".into()),
    });
    harness.run_steps(4);
    assert_eq!(harness.query_all_by_label("保护").count(), 3);
    assert_eq!(harness.query_all_by_label("不保护").count(), 1);
    assert_eq!(harness.query_all_by_label("待重启生效").count(), 3);

    // 直接拨动 C: 行开关取消保护（保护 -> off(待重启生效) -> 不保护）
    harness.get_by_label("切换 C: 重启后保护").click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Unprotect 命令");
    };
    assert!(matches!(cmd, BackendCmd::Unprotect(ref drive) if drive == "C:"));

    updated.volumes[0].next_protected = false;
    let _ = event_tx.send(BackendEvent::Snapshot(updated));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("C: 已取消保护".into()),
    });
    harness.run_steps(4);
    assert_eq!(harness.query_all_by_label("保护").count(), 2);
    assert_eq!(harness.query_all_by_label("不保护").count(), 2);
    assert_eq!(harness.query_all_by_label("待重启生效").count(), 4);
}

#[test]
fn gui_cache_form_validation_and_save() {
    let (mut harness, _event_tx, cmd_rx) = build_gui_harness(
        BackendKind::Mock,
        StubFileDialog::default(),
        Some(demo_snapshot()),
    );

    {
        let warn_input = harness.get_by_label("警告阈值");
        warn_input.click();
    }
    harness.state_mut().cache.warning_text = "2000".into();
    harness.state_mut().cache.on_edited();
    harness.run();

    harness.get_by_label("应用缓存设置").click();
    harness.run();
    assert!(cmd_rx.try_recv().is_err());
    assert!(
        harness
            .query_all_by_label_contains("警告阈值必须小于严重阈值")
            .next()
            .is_some()
    );

    harness.get_by_label("还原").click();
    harness.run();
    assert_eq!(harness.state().cache.warning_text, "1536");

    harness.state_mut().cache.warning_text = "1530".into();
    harness.state_mut().cache.on_edited();
    harness.run();
    harness.get_by_label("应用缓存设置").click();
    harness.run_steps(2);
    match recv_cmd(&cmd_rx) {
        BackendCmd::SetOverlayCache {
            max_mb,
            warning_mb,
            critical_mb,
        } => assert_eq!((max_mb, warning_mb, critical_mb), (2048, 1530, 1843)),
        other => panic!("期望 SetOverlayCache，实际 {other:?}"),
    }
}

#[test]
fn gui_folder_exclusion_with_stub_file_dialog() {
    let stub = StubFileDialog::with_replies([Some(PathBuf::from("D:\\TempWork"))]);
    let (mut harness, _event_tx, cmd_rx) =
        build_gui_harness(BackendKind::Mock, stub, Some(demo_snapshot()));

    harness.get_by_label("+ 文件夹").click();
    harness.run();
    harness.get_by_label("选择目录…").click();
    harness.run();
    harness.get_by_label("确认").click();
    harness.run_steps(2);

    match recv_cmd(&cmd_rx) {
        BackendCmd::AddFileExclusion(path) => assert_eq!(path, "D:\\TempWork"),
        other => panic!("期望 AddFileExclusion，实际 {other:?}"),
    }
}

#[test]
fn gui_commit_file_deletion_enforces_two_step_confirm() {
    let stub = StubFileDialog::with_replies([Some(PathBuf::from("C:\\data\\a.txt"))]);
    let (mut harness, _event_tx, cmd_rx) =
        build_gui_harness(BackendKind::Mock, stub, Some(demo_snapshot()));

    harness.get_by_label("强制保存…").click();
    harness.run();
    harness.get_by_label("选择待删文件…").click();
    harness.run();
    harness.get_by_label("浏览文件…").click();
    harness.run();
    harness.get_by_label("下一步").click();
    harness.run();
    // 第一步仅切到二次确认，尚未发命令
    assert!(cmd_rx.try_recv().is_err());

    harness.get_by_label("确认永久删除").click();
    harness.run_steps(2);
    match recv_cmd(&cmd_rx) {
        BackendCmd::CommitFileDeletion(path) => assert_eq!(path, "C:\\data\\a.txt"),
        other => panic!("期望 CommitFileDeletion，实际 {other:?}"),
    }
}

#[test]
fn gui_not_installed_and_logs_modal() {
    let (mut harness, event_tx, cmd_rx) =
        build_gui_harness(BackendKind::Mock, StubFileDialog::default(), None);

    let _ = event_tx.send(BackendEvent::OsVersion(
        "Windows 10 Pro (Build 19045, 64-bit)".into(),
    ));
    let _ = event_tx.send(BackendEvent::NotInstalled(None));
    harness.run();
    assert!(harness.query_by_label_contains("Windows 10 Pro").is_some());
    assert!(
        harness
            .query_by_label_contains("当前非企业版或教育版")
            .is_some()
    );
    assert!(
        harness
            .query_by_label_contains("需要 Windows 企业版和教育版才能使用 UWF 功能")
            .is_some()
    );
    let _ = event_tx.send(BackendEvent::OsVersion(
        "Windows 10 Enterprise LTSC 2021 16G".into(),
    ));
    harness.run();
    assert!(
        harness
            .query_by_label_contains("版本符合要求：已支持启用 UWF 功能")
            .is_some()
    );
    harness.get_by_label("重新检测").click();
    harness.run_steps(2);
    assert!(matches!(cmd_rx.try_recv(), Ok(BackendCommand::RefreshAll)));

    let _ = event_tx.send(BackendEvent::Snapshot(demo_snapshot()));
    harness.run();
    harness.get_by_label_contains("日志 (").click();
    harness.run();
    assert!(matches!(harness.state().modal, Some(Modal::Logs)));
    assert!(
        harness
            .query_by_label_contains("操作记录（最新在前")
            .is_some()
    );
    assert!(harness.query_by_label("复制全部").is_some());
    harness.get_by_label("关闭").click();
    harness.run();
    assert!(harness.state().modal.is_none());

    // 打开帮助弹窗并验证包含 GitHub 地址
    harness.get_by_label("帮助 (F1)").click();
    harness.run();
    assert!(
        harness
            .query_by_label("https://github.com/GreyRaphael/uwf_manager")
            .is_some()
    );
    harness.get_by_label("知道了").click();
    harness.run();
    assert!(harness.state().modal.is_none());
}

#[test]
fn gui_commit_preflight_blocks_unprotected_drive_and_disabled_filter() {
    let stub = StubFileDialog::with_replies([
        Some(PathBuf::from("D:\\unprotected.txt")),
        Some(PathBuf::from("D:\\unprotected.txt")),
    ]);
    let (mut harness, event_tx, cmd_rx) =
        build_gui_harness(BackendKind::Mock, stub, Some(demo_snapshot()));

    // 仅保护 C: 时，选 D: 永久删除在第 1 步即内联拦截，不进入第 2 步也不发后端命令
    harness.get_by_label("强制保存…").click();
    harness.run();
    harness.get_by_label("选择待删文件…").click();
    harness.run();
    harness.get_by_label("浏览文件…").click();
    harness.run();
    harness.get_by_label("下一步").click();
    harness.run();
    assert!(cmd_rx.try_recv().is_err());
    assert!(
        harness
            .query_by_label_contains("卷 D: 当前会话未受 UWF 保护")
            .is_some()
    );
    assert!(harness.query_by_label("确认永久删除").is_none());
    harness.get_by_label("取消").click();
    harness.run();

    // 筛选器当前会话未启用时，提交 C: 文件同样被内联拦截
    let mut disabled_snap = demo_snapshot();
    disabled_snap.filter_state = Some(FilterState {
        current_enabled: false,
        next_enabled: false,
    });
    let _ = event_tx.send(BackendEvent::Snapshot(disabled_snap));
    harness.run();
    assert!(
        harness
            .state()
            .validate_input_preflight(InputAction::CommitFile, "C:\\data\\a.txt")
            .is_err_and(|e| e.contains("筛选器当前会话未启用"))
    );

    // 即使直接绕过弹窗传未受保护盘符给强制保存文件修改，也会被预检拦下并写红字错误
    harness.get_by_label("强制保存…").click();
    harness.run();
    assert!(harness.query_by_label("强制保存文件修改").is_some());
    assert!(harness.query_by_label("强制保存注册表键值").is_some());
    harness.get_by_label("选择文件…").click();
    harness.run();
    assert!(harness.query_by_label("强制保存文件修改").is_some());
    harness.get_by_label("浏览文件…").click();
    harness.run();
    harness.get_by_label("确认").click();
    harness.run();
    assert!(cmd_rx.try_recv().is_err());
    assert!(
        harness
            .query_by_label_contains("筛选器当前会话未启用")
            .is_some()
    );
}

#[test]
fn gui_top_bar_os_panel_exclusions_and_revert() {
    let (mut harness, event_tx, cmd_rx) = build_gui_harness(
        BackendKind::Mock,
        StubFileDialog::default(),
        Some(demo_snapshot()),
    );

    // 1. 顶栏去掉「WMI 实机 / Mock 演示」，左边界右移到右侧工作区上方，「日志」位于「帮助」左侧；
    //    同时「筛选器控制」卡片上移到左侧顶部与顶栏齐平，缓存区域去掉「可用剩余空间」
    assert!(harness.query_by_label("WMI 实机").is_none());
    assert!(harness.query_by_label("Mock 演示").is_none());
    assert!(harness.query_by_label_contains("可用剩余空间").is_none());

    let logs_rect = harness.get_by_label_contains("日志 (").rect();
    let help_rect = harness.get_by_label("帮助 (F1)").rect();
    let commit_rect = harness.get_by_label("强制保存…").rect();
    let filter_hdr_rect = harness.get_by_label("写保护总开关").rect();
    assert!(
        logs_rect.max.x <= help_rect.min.x,
        "日志按钮应位于帮助按钮左侧: logs={logs_rect:?}, help={help_rect:?}"
    );
    assert!(
        filter_hdr_rect.min.x < 350.0,
        "筛选器控制应位于左侧栏: filter={filter_hdr_rect:?}"
    );
    assert!(
        (filter_hdr_rect.center().y - logs_rect.center().y).abs() < 40.0,
        "筛选器控制应向上移至顶栏齐平位置: filter={filter_hdr_rect:?}, logs={logs_rect:?}"
    );
    assert!(
        commit_rect.min.x > filter_hdr_rect.max.x + 60.0,
        "顶栏左边界应右移让出左侧栏顶部空间: commit={commit_rect:?}, filter={filter_hdr_rect:?}"
    );

    // 2. 左下角系统环境面板展示带物理内存后缀的版本文本与企业版/教育版提示，且位于工作区下半部分偏左
    assert!(harness.query_by_label_contains("(Build").is_none());
    assert!(harness.query_by_label_contains("64-bit").is_none());
    let os_node_rect = harness
        .get_by_label_contains("Windows 10 Enterprise LTSC 2021 16G")
        .rect();
    assert!(os_node_rect.min.x < 350.0);
    assert!(os_node_rect.min.y > 450.0);
    assert!(
        harness
            .query_by_label_contains("需要 Windows 企业版和教育版才能使用 UWF 功能")
            .is_some()
    );

    // 3. 排除项规则去掉了全部/文件与目录/注册表筛选开关，也去掉了双会话视角切换；列表行仍保留类型徽章，工具栏单行不换行
    assert!(harness.query_by_label("全部").is_none());
    assert!(harness.query_by_label("文件与目录").is_none());
    assert!(harness.query_by_label("当前生效 (只读)").is_none());
    assert!(harness.query_by_label("重启后生效 (可编辑)").is_none());
    assert!(harness.query_by_label("文件/目录").is_some());
    assert!(harness.query_by_label("注册表").is_some());
    let add_file_rect = harness.get_by_label("+ 文件").rect();
    let revert_rect = harness.get_by_label("撤销修改 (2)").rect();
    assert!(
        (add_file_rect.center().y - revert_rect.center().y).abs() < 4.0,
        "排除项工具栏按钮不应换行: add={add_file_rect:?}, revert={revert_rect:?}"
    );
    let import_rect = harness.get_by_label("导入列表").rect();
    let export_rect = harness.get_by_label("导出列表").rect();
    assert!(
        (import_rect.center().y - export_rect.center().y).abs() < 4.0,
        "导入导出按钮应在标题栏同一水平行: import={import_rect:?}, export={export_rect:?}"
    );

    assert!(harness.query_by_label("当前设置已全部生效").is_none());
    assert!(harness.query_by_label("⚠ 待重启生效").is_some());
    assert!(harness.query_by_label("立即重启生效…").is_some());

    // 4. 保护缓存细进度条上方显示用量与阈值百分比提示，分区表改为 5 列，每行支持直觉移除排除项
    assert!(harness.query_by_label("512 / 2048 MB").is_some());
    assert!(harness.query_by_label("当前状态").is_some());
    assert!(harness.query_by_label("开关").is_some());
    assert!(harness.query_by_label("重启后状态").is_some());
    assert!(harness.query_by_label("保持不变").is_none());
    assert_eq!(harness.query_all_by_label("保护").count(), 2);
    assert_eq!(harness.query_all_by_label("不保护").count(), 2);
    assert!(harness.query_by_label_contains("(75%)").is_some());
    assert!(harness.query_by_label_contains("(90%)").is_some());
    assert!(harness.query_by_label("移除选中").is_none());
    assert_eq!(harness.query_all_by_label("移除").count(), 2);

    harness.get_by_label("撤销修改 (2)").click();
    harness.run();
    assert!(harness.query_by_label_contains("预计移除 2 项").is_some());
    assert!(
        harness
            .query_by_label_contains("将移除（重启后新增）：")
            .is_some()
    );
    assert!(
        harness
            .query_all_by_label_contains("C:\\Windows\\Temp")
            .count()
            >= 2
    );
    harness.get_by_label("确认撤销").click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::RevertExclusionsToCurrent));

    // 撤销完成刷新为与当前一致后，差异清零，顶栏显示「当前设置已全部生效」
    let mut aligned = demo_snapshot();
    aligned.file_exclusions_next = aligned.file_exclusions_current.clone();
    aligned.registry_exclusions_next = aligned.registry_exclusions_current.clone();
    let _ = event_tx.send(BackendEvent::Snapshot(aligned));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("已撤销排除项修改".into()),
    });
    harness.run();
    assert!(harness.query_by_label("⚠ 待重启生效").is_none());
    assert!(harness.query_by_label("✔ 当前设置已全部生效").is_some());

    // 5. 也测试点击「立即重启生效…」会弹出重启确认对话框
    harness.get_by_label("切换写保护总开关").click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Mutate 命令");
    };
    assert!(matches!(cmd, BackendCmd::DisableFilter));
    let mut pending_snap = demo_snapshot();
    pending_snap.file_exclusions_next.clear();
    pending_snap.registry_exclusions_next.clear();
    pending_snap.filter_state = Some(FilterState {
        current_enabled: true,
        next_enabled: false,
    });
    let _ = event_tx.send(BackendEvent::Snapshot(pending_snap));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("将在重启后停用保护".into()),
    });
    harness.run();
    assert!(
        harness
            .query_by_label_contains("将在重启后停用保护")
            .is_some()
    );
    harness.get_by_label("立即重启生效…").click();
    harness.run();
    assert!(harness.query_by_label("重启系统确认").is_some());
    harness.get_by_label("取消").click();
    harness.run();

    // 恢复一个非空排除项并通过行内「移除」按钮直接移除第一行
    let _ = event_tx.send(BackendEvent::Snapshot(demo_snapshot()));
    harness.run();
    harness.query_all_by_label("移除").next().unwrap().click();
    harness.run_steps(2);
    let BackendCommand::Mutate { id, cmd } = cmd_rx.try_recv().unwrap() else {
        panic!("期望 Remove 命令");
    };
    assert!(matches!(cmd, BackendCmd::RemoveFileExclusion(_)));
    let _ = event_tx.send(BackendEvent::Finished {
        id,
        outcome: Outcome::Ok("已移除排除项".into()),
    });
    harness.run();
    assert!(harness.state().modal.is_none());
}
