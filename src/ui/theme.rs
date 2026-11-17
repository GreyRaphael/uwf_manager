//! Fluent 亮色桌面主题与可复用控件样式。

use eframe::egui::{
    self, Align2, Button, Color32, CornerRadius, FontId, Frame, Margin, Response, RichText, Stroke,
    Ui, Vec2, Visuals,
};

use crate::app::LogLevel;
use crate::model::UsageLevel;

// ── 调色板（高对比、克制亮色桌面风格） ──
pub const BG_CANVAS: Color32 = Color32::from_rgb(243, 245, 249); // #F3F5F9
pub const BG_CARD: Color32 = Color32::WHITE;
pub const BG_SUBTLE: Color32 = Color32::from_rgb(248, 250, 252); // #F8FAFC
pub const BG_HOVER: Color32 = Color32::from_rgb(238, 242, 247); // #EEF2F7
pub const BORDER: Color32 = Color32::from_rgb(226, 232, 240); // #E2E8F0
pub const BORDER_STRONG: Color32 = Color32::from_rgb(203, 213, 225); // #CBD5E1

pub const ACCENT: Color32 = Color32::from_rgb(0, 102, 204); // #0066CC
pub const ACCENT_SOFT: Color32 = Color32::from_rgb(232, 242, 255);

pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(15, 23, 42); // #0F172A
pub const TEXT_SECONDARY: Color32 = Color32::from_rgb(71, 85, 105); // #475569
pub const TEXT_MUTED: Color32 = Color32::from_rgb(100, 116, 139); // #64748B

pub const OK_FG: Color32 = Color32::from_rgb(22, 101, 52); // #166534
pub const OK_BG: Color32 = Color32::from_rgb(220, 252, 231); // #DCFCE7
pub const OK_BAR: Color32 = Color32::from_rgb(22, 163, 74); // #16A34A

pub const WARN_FG: Color32 = Color32::from_rgb(180, 83, 9); // #B45309
pub const WARN_BG: Color32 = Color32::from_rgb(254, 243, 199); // #FEF3C7
pub const WARN_BAR: Color32 = Color32::from_rgb(217, 119, 6); // #D97706

pub const ERR_FG: Color32 = Color32::from_rgb(185, 28, 28); // #B91C1C
pub const ERR_BG: Color32 = Color32::from_rgb(254, 226, 226); // #FEE2E2
pub const ERR_BAR: Color32 = Color32::from_rgb(220, 38, 38); // #DC2626

pub const INFO_BG: Color32 = Color32::from_rgb(239, 246, 255); // #EFF6FF
pub const INFO_FG: Color32 = Color32::from_rgb(29, 78, 216); // #1D4ED8

/// 表单左侧固定标签宽度（保证「缓存位置」与下方输入行左侧标签对齐）。
pub const FIELD_LABEL_WIDTH: f32 = 78.0;

/// 初始化全局浅色 Fluent 视觉与排版节奏。
pub fn apply_theme(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Light);
    ctx.style_mut_of(egui::Theme::Light, |style| {
        let mut visuals = Visuals::light();

        visuals.panel_fill = BG_CANVAS;
        visuals.window_fill = BG_CARD;
        visuals.extreme_bg_color = BG_CARD;
        visuals.faint_bg_color = BG_SUBTLE;
        visuals.code_bg_color = BG_SUBTLE;
        visuals.override_text_color = Some(TEXT_PRIMARY);

        visuals.window_stroke = Stroke::new(1.0, BORDER_STRONG);
        visuals.window_corner_radius = CornerRadius::same(10);

        visuals.selection.bg_fill = ACCENT;
        visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

        visuals.widgets.noninteractive.bg_fill = BG_CARD;
        visuals.widgets.noninteractive.weak_bg_fill = BG_SUBTLE;
        visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
        visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        visuals.widgets.noninteractive.corner_radius = CornerRadius::same(6);

        visuals.widgets.inactive.bg_fill = BG_CARD;
        visuals.widgets.inactive.weak_bg_fill = BG_SUBTLE;
        visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        visuals.widgets.inactive.corner_radius = CornerRadius::same(6);

        visuals.widgets.hovered.bg_fill = BG_HOVER;
        visuals.widgets.hovered.weak_bg_fill = BG_HOVER;
        visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, ACCENT);
        visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, TEXT_PRIMARY);
        visuals.widgets.hovered.corner_radius = CornerRadius::same(6);

        visuals.widgets.active.bg_fill = ACCENT_SOFT;
        visuals.widgets.active.weak_bg_fill = ACCENT_SOFT;
        visuals.widgets.active.bg_stroke = Stroke::new(1.5, ACCENT);
        visuals.widgets.active.fg_stroke = Stroke::new(1.0, ACCENT);
        visuals.widgets.active.corner_radius = CornerRadius::same(6);

        visuals.striped = true;
        // 关闭亚像素横向分桶，减少字形图集变体数量与内存占用（配合 1024 图集上限）
        visuals.text_options.subpixel_binning = false;

        style.visuals = visuals;
        style.spacing.item_spacing = Vec2::new(8.0, 8.0);
        style.spacing.button_padding = Vec2::new(11.0, 5.0);
        style.spacing.window_margin = Margin::same(16);
        style.spacing.indent = 14.0;
    });
}

/// 标准白底圆角卡片框架。
pub fn card_frame() -> Frame {
    Frame::NONE
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin::symmetric(14, 11))
}

/// 弹窗卡片外框：白底、大圆角、柔和边框与内边距。
pub fn modal_frame() -> Frame {
    Frame::NONE
        .fill(BG_CARD)
        .stroke(Stroke::new(1.0, BORDER_STRONG))
        .corner_radius(CornerRadius::same(12))
        .inner_margin(Margin::same(20))
}

/// 卡片头部标题 + 下方可选说明文字（避免在窄边栏与右侧控件争抢宽度）。
pub fn card_header(ui: &mut Ui, title: &str, subtitle: Option<&str>) {
    card_header_with_action(ui, title, subtitle, |_| {});
}

/// 带右侧扩展操作区的卡片头部（左侧标题，右侧可选操作区，下方可选说明文字）。
pub fn card_header_with_action(
    ui: &mut Ui,
    title: &str,
    subtitle: Option<&str>,
    right_action: impl FnOnce(&mut Ui),
) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(FontId::proportional(15.0))
                .strong()
                .color(TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            right_action(ui);
        });
    });
    if let Some(sub) = subtitle {
        ui.add_space(1.0);
        ui.add(
            egui::Label::new(
                RichText::new(sub)
                    .font(FontId::proportional(11.5))
                    .color(TEXT_MUTED),
            )
            .wrap(),
        );
    }
}

/// 高优先级实心强调按钮（禁用态转为低对比灰底以免误导点击）。
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let enabled = ui.is_enabled();
    let (fill, border, fg) = if enabled {
        (ACCENT, ACCENT, Color32::WHITE)
    } else {
        (BG_SUBTLE, BORDER, TEXT_MUTED)
    };
    ui.add(
        Button::new(RichText::new(label).color(fg).strong())
            .fill(fill)
            .stroke(Stroke::new(1.0, border))
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 常规描边次级按钮。
pub fn secondary_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        Button::new(RichText::new(label).color(TEXT_PRIMARY))
            .fill(BG_SUBTLE)
            .stroke(Stroke::new(1.0, BORDER_STRONG))
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 无边框轻量幽灵按钮（用于日志、帮助等辅助信息入口）。
pub fn ghost_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        Button::new(RichText::new(label).color(TEXT_SECONDARY))
            .fill(Color32::TRANSPARENT)
            .stroke(Stroke::NONE)
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 克制的柔和危险操作按钮。
pub fn danger_button(ui: &mut Ui, label: &str) -> Response {
    ui.add(
        Button::new(RichText::new(label).color(ERR_FG))
            .fill(ERR_BG)
            .stroke(Stroke::new(1.0, Color32::from_rgb(252, 165, 165)))
            .corner_radius(CornerRadius::same(6))
            .min_size(Vec2::new(0.0, 28.0)),
    )
}

/// 统一风格下拉菜单按钮（次级描边风格与内边距，内置矢量下拉箭头以避免字体缺字）。
pub fn menu_button<R>(ui: &mut Ui, label: &str, add_contents: impl FnOnce(&mut Ui) -> R) {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = Vec2::new(11.0, 6.0);
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = BG_SUBTLE;
        w.inactive.bg_stroke = Stroke::new(1.0, BORDER_STRONG);
        let chevron = egui::Atom::paint(Vec2::new(8.0, 8.0), |ui, args| {
            let c = args.rect.center();
            let color = if ui.is_enabled() {
                TEXT_SECONDARY
            } else {
                TEXT_MUTED
            };
            let stroke = Stroke::new(1.4, color);
            ui.painter().line_segment(
                [egui::pos2(c.x - 3.0, c.y - 1.0), egui::pos2(c.x, c.y + 2.0)],
                stroke,
            );
            ui.painter().line_segment(
                [egui::pos2(c.x, c.y + 2.0), egui::pos2(c.x + 3.0, c.y - 1.0)],
                stroke,
            );
        });
        let button = Button::new((RichText::new(label).color(TEXT_PRIMARY), chevron))
            .min_size(Vec2::new(0.0, 28.0));
        egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents);
    });
}

/// 实测当前主题/DPI 下单行文本的渲染宽度，避免固定像素预留导致缩放截断。
pub fn measure_text_width(
    ui: &Ui,
    text: impl Into<egui::WidgetText>,
    fallback_style: egui::TextStyle,
) -> f32 {
    text.into()
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Extend),
            f32::INFINITY,
            fallback_style,
        )
        .size()
        .x
}

/// 实测标准按钮占用宽度（用于左侧输入框与右侧按钮同一水平行精确分配剩余宽度）。
pub fn measure_button_width(ui: &Ui, label: &str) -> f32 {
    measure_text_width(ui, label, egui::TextStyle::Button)
        + ui.spacing().button_padding.x * 2.0
        + 4.0
}

fn pill_frame(bg: Color32, margin: Margin) -> Frame {
    Frame::NONE
        .fill(bg)
        .corner_radius(CornerRadius::same(10))
        .inner_margin(margin)
}

/// 语义胶囊标签（状态/徽章/计数）。
pub fn status_pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    pill_frame(bg, Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.label(
            RichText::new(text)
                .font(FontId::proportional(11.5))
                .color(fg)
                .strong(),
        );
    });
}

/// 带矢量圆点的语义状态胶囊（避免依赖字库中的圆点字符）。
pub fn status_dot_pill(ui: &mut Ui, text: &str, fg: Color32, bg: Color32) {
    pill_frame(bg, Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(7.0), egui::Sense::hover());
            if ui.is_rect_visible(rect) {
                ui.painter().circle_filled(rect.center(), 3.0, fg);
            }
            ui.label(
                RichText::new(text)
                    .font(FontId::proportional(11.5))
                    .color(fg)
                    .strong(),
            );
        });
    });
}

/// 双态（当前 / 重启后）布尔状态胶囊：待重启变更时琥珀高亮，否则按激活与否展示绿/灰胶囊。
pub fn state_status_pill(
    ui: &mut Ui,
    active: bool,
    pending: bool,
    on_label: &str,
    off_label: &str,
) {
    let (fg, bg) = if pending {
        (WARN_FG, WARN_BG)
    } else if active {
        (OK_FG, OK_BG)
    } else {
        (TEXT_SECONDARY, BG_SUBTLE)
    };
    let label = if active { on_label } else { off_label };
    status_pill(ui, label, fg, bg);
}

/// 现代分段胶囊切换控件，返回是否切换了选中项。
pub fn segmented_toggle<T: Copy + PartialEq>(
    ui: &mut Ui,
    current: &mut T,
    options: &[(T, &str)],
) -> bool {
    let mut changed = false;
    Frame::NONE
        .fill(BG_SUBTLE)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(CornerRadius::same(7))
        .inner_margin(Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (val, label) in options {
                    let selected = *current == *val;
                    let fill = if selected {
                        BG_CARD
                    } else {
                        Color32::TRANSPARENT
                    };
                    let stroke = if selected {
                        Stroke::new(1.0, BORDER_STRONG)
                    } else {
                        Stroke::NONE
                    };
                    let fg = if selected { ACCENT } else { TEXT_SECONDARY };
                    let text = RichText::new(*label)
                        .font(FontId::proportional(12.5))
                        .color(fg);
                    let text = if selected { text.strong() } else { text };
                    if ui
                        .add(
                            Button::new(text)
                                .fill(fill)
                                .stroke(stroke)
                                .corner_radius(CornerRadius::same(5)),
                        )
                        .clicked()
                        && !selected
                    {
                        *current = *val;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// 带左侧固定标签、末尾对齐单位列与无障碍关联的单行文本输入。
pub fn labeled_input_row(
    ui: &mut Ui,
    label: &str,
    value: &mut String,
    unit: Option<&str>,
    unit_reserve_w: f32,
    enabled: bool,
) -> Response {
    ui.horizontal(|ui| {
        let lbl = ui.add_sized(
            [FIELD_LABEL_WIDTH, 24.0],
            egui::Label::new(
                RichText::new(label)
                    .font(FontId::proportional(12.5))
                    .color(if enabled { TEXT_PRIMARY } else { TEXT_MUTED }),
            ),
        );
        let spacing = ui.spacing().item_spacing.x;
        let reserved_unit = if unit.is_some() {
            unit_reserve_w + spacing
        } else {
            0.0
        };
        let input_w = (ui.available_width() - reserved_unit).max(64.0);
        let resp = ui
            .add_enabled(
                enabled,
                egui::TextEdit::singleline(value)
                    .desired_width(input_w)
                    .align(Align2::LEFT_CENTER),
            )
            .labelled_by(lbl.id);
        if let Some(unit) = unit {
            ui.label(
                RichText::new(unit)
                    .font(FontId::proportional(12.0))
                    .color(TEXT_MUTED),
            );
        }
        resp
    })
    .inner
}

/// 缓存健康度对应颜色组合（文字前景色、进度条填充色、说明文案）。
pub fn usage_colors(level: UsageLevel) -> (Color32, Color32, &'static str) {
    match level {
        UsageLevel::Normal => (OK_FG, OK_BAR, "健康"),
        UsageLevel::Warning => (WARN_FG, WARN_BAR, "预警"),
        UsageLevel::Critical => (ERR_FG, ERR_BAR, "临界"),
    }
}

fn log_level_colors(level: LogLevel) -> (Color32, Color32) {
    match level {
        LogLevel::Ok => (OK_FG, OK_BG),
        LogLevel::Warn => (WARN_FG, WARN_BG),
        LogLevel::Err => (ERR_FG, ERR_BG),
    }
}

/// 用 `Painter` 绘制状态矢量图标（对钩/感叹三角/叉号），避免依赖 Segoe UI Symbol 额外字体。
fn paint_status_icon(
    painter: &egui::Painter,
    center: egui::Pos2,
    size: f32,
    level: LogLevel,
    color: Color32,
) {
    let r = size * 0.42;
    let stroke = Stroke::new(1.5, color);
    match level {
        LogLevel::Ok => {
            let p1 = center + Vec2::new(-r * 0.68, r * 0.05);
            let p2 = center + Vec2::new(-r * 0.14, r * 0.58);
            let p3 = center + Vec2::new(r * 0.72, -r * 0.52);
            painter.line_segment([p1, p2], stroke);
            painter.line_segment([p2, p3], stroke);
        }
        LogLevel::Warn => {
            let top = center + Vec2::new(0.0, -r * 0.80);
            let left = center + Vec2::new(-r * 0.86, r * 0.72);
            let right = center + Vec2::new(r * 0.86, r * 0.72);
            let tri_stroke = Stroke::new(1.25, color);
            painter.line_segment([top, right], tri_stroke);
            painter.line_segment([right, left], tri_stroke);
            painter.line_segment([left, top], tri_stroke);
            painter.line_segment(
                [
                    center + Vec2::new(0.0, -r * 0.28),
                    center + Vec2::new(0.0, r * 0.22),
                ],
                tri_stroke,
            );
            painter.circle_filled(center + Vec2::new(0.0, r * 0.46), 0.9, color);
        }
        LogLevel::Err => {
            let d = r * 0.58;
            painter.line_segment(
                [center + Vec2::new(-d, -d), center + Vec2::new(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(d, -d), center + Vec2::new(-d, d)],
                stroke,
            );
        }
    }
}

/// 纯矢量日志等级徽章（替代字符 `✔`/`⚠`/`✖`）。
pub fn log_status_badge(ui: &mut Ui, level: LogLevel) {
    let (fg, bg) = log_level_colors(level);
    pill_frame(bg, Margin::symmetric(6, 3)).show(ui, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(11.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            paint_status_icon(ui.painter(), rect.center(), rect.width(), level, fg);
        }
    });
}

/// 带矢量图标的状态胶囊。
pub fn icon_status_pill(ui: &mut Ui, level: LogLevel, text: &str, fg: Color32, bg: Color32) {
    pill_frame(bg, Margin::symmetric(8, 2)).show(ui, |ui| {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(11.0), egui::Sense::hover());
            if ui.is_rect_visible(rect) {
                paint_status_icon(ui.painter(), rect.center(), rect.width(), level, fg);
            }
            ui.label(
                RichText::new(text)
                    .font(FontId::proportional(11.5))
                    .color(fg)
                    .strong(),
            );
        });
    });
}

/// 带矢量图标的状态提示文本（无底色胶囊背景）。
pub fn status_hint_with_icon(ui: &mut Ui, level: LogLevel, text: &str, color: Color32) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
        if ui.is_rect_visible(rect) {
            paint_status_icon(ui.painter(), rect.center(), rect.width(), level, color);
        }
        ui.label(
            RichText::new(text)
                .font(FontId::proportional(12.0))
                .color(color),
        );
    });
}

/// Fluent 风格细进度条（避免文字压在填充交界处影响对比度）。
pub fn slim_progress_bar(ui: &mut Ui, ratio: f32, fill_color: Color32) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width, 10.0), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return;
    }

    let radius = rect.height() * 0.5;
    let painter = ui.painter();
    painter.rect_filled(rect, radius, BG_SUBTLE);
    painter.rect_stroke(
        rect,
        radius,
        Stroke::new(1.0, BORDER),
        egui::StrokeKind::Inside,
    );

    let clamped = ratio.clamp(0.0, 1.0);
    if clamped > 0.0 {
        let min_visible_w = if clamped > 0.01 { rect.height() } else { 0.0 };
        let fill_w = (rect.width() * clamped)
            .max(min_visible_w)
            .min(rect.width());
        let fill_rect = egui::Rect::from_min_size(rect.min, Vec2::new(fill_w, rect.height()));
        painter.rect_filled(fill_rect, radius, fill_color);
    }
}

/// Fluent 风格布尔开关（用于分区保护等行内直接切换），状态以 `on` 为唯一真相源。
pub fn toggle_switch(ui: &mut Ui, on: bool, a11y_label: &str) -> Response {
    let desired_size = Vec2::new(36.0, 20.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::Switch, ui.is_enabled(), on, a11y_label)
    });

    if ui.is_rect_visible(rect) {
        let how_on = ui.ctx().animate_bool_responsive(response.id, on);
        let enabled = ui.is_enabled();
        let hovered = enabled && (response.hovered() || response.has_focus());

        let off_bg = if enabled { BORDER_STRONG } else { BORDER };
        let on_bg = if enabled {
            if hovered {
                Color32::from_rgb(0, 86, 179)
            } else {
                ACCENT
            }
        } else {
            Color32::from_rgb(147, 197, 253)
        };
        let bg = off_bg.lerp_to_gamma(on_bg, how_on);

        let painter = ui.painter();
        let radius = 0.5 * rect.height();
        if hovered {
            let halo = Color32::from_rgba_unmultiplied(ACCENT.r(), ACCENT.g(), ACCENT.b(), 80);
            painter.rect_stroke(
                rect,
                radius,
                Stroke::new(2.0, halo),
                egui::StrokeKind::Outside,
            );
        }
        painter.rect_filled(rect, radius, bg);

        let circle_x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), how_on);
        let center = egui::pos2(circle_x, rect.center().y);
        let knob_color = if enabled { Color32::WHITE } else { BG_SUBTLE };
        let knob_r = 0.72 * radius;
        painter.circle_filled(center, knob_r, knob_color);
        painter.circle_stroke(
            center,
            knob_r,
            Stroke::new(0.6, Color32::from_rgba_unmultiplied(15, 23, 42, 36)),
        );
    }

    response
}
