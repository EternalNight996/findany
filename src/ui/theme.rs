//! 主题与字体（深/浅双色，对齐 Python 版 build_qss 的配色与字号意图）。
//!
//! v2 起整体字号上调一档（正文 15px / 小字 12.5px），所有界面文本走这里定义的大小，
//! 不再散落硬编码 —— 要整体缩放只改 \`SIZE_*\` 常量。

use eframe::egui::{self, Color32, FontData, FontDefinitions, FontFamily, FontId, TextStyle};
use std::sync::Arc;

// ---- 字号档位（单点可调）----
pub const SIZE_BODY: f32 = 15.0;
pub const SIZE_SMALL: f32 = 12.5;
pub const SIZE_HEAD: f32 = 16.0;
pub const SIZE_TITLE: f32 = 18.0;
pub const SIZE_STAT: f32 = 19.0;
pub const SIZE_MONO: f32 = 14.0;

// ---- 行高/圆角档位 ----
pub const ROW_H: f32 = 26.0;
pub const INPUT_H: f32 = 28.0;
pub const BTN_H: f32 = 30.0;
pub const RADIUS: u8 = 6;

#[derive(Clone, Copy)]
pub struct Palette {
    pub bg: Color32,
    pub panel: Color32,
    pub panel2: Color32,
    pub card: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text2: Color32,
    pub brand: Color32,
    pub brand_d: Color32,
    pub ok: Color32,
    pub warn: Color32,
    pub err: Color32,
    pub info: Color32,
}

pub fn dark() -> Palette {
    Palette {
        bg: Color32::from_rgb(0x12, 0x14, 0x18),
        panel: Color32::from_rgb(0x1a, 0x1d, 0x23),
        panel2: Color32::from_rgb(0x22, 0x26, 0x2e),
        card: Color32::from_rgb(0x1e, 0x22, 0x29),
        border: Color32::from_rgb(0x30, 0x36, 0x40),
        text: Color32::from_rgb(0xea, 0xed, 0xf1),
        text2: Color32::from_rgb(0x9a, 0xa2, 0xad),
        brand: Color32::from_rgb(0x56, 0x86, 0xfe),
        brand_d: Color32::from_rgb(0x3b, 0x66, 0xd6),
        ok: Color32::from_rgb(0x2e, 0xc4, 0x6a),
        warn: Color32::from_rgb(0xf5, 0x9e, 0x0b),
        err: Color32::from_rgb(0xef, 0x44, 0x44),
        info: Color32::from_rgb(0x56, 0x86, 0xfe),
    }
}

pub fn light() -> Palette {
    Palette {
        bg: Color32::from_rgb(0xf4, 0xf5, 0xf7),
        panel: Color32::from_rgb(0xff, 0xff, 0xff),
        panel2: Color32::from_rgb(0xec, 0xee, 0xf2),
        card: Color32::from_rgb(0xfa, 0xfb, 0xfc),
        border: Color32::from_rgb(0xd4, 0xd9, 0xe0),
        text: Color32::from_rgb(0x1a, 0x1e, 0x24),
        text2: Color32::from_rgb(0x66, 0x6e, 0x79),
        brand: Color32::from_rgb(0x2f, 0x6f, 0xed),
        brand_d: Color32::from_rgb(0x1f, 0x55, 0xc4),
        ok: Color32::from_rgb(0x14, 0x9e, 0x47),
        warn: Color32::from_rgb(0xb8, 0x6c, 0x00),
        err: Color32::from_rgb(0xd1, 0x2b, 0x2b),
        info: Color32::from_rgb(0x2f, 0x6f, 0xed),
    }
}

/// 装载中文字体（雅黑 > 黑体 > 宋体；Linux 走 Noto/文泉驿），返回命中的字体名。
pub fn install_fonts(ctx: &egui::Context) -> String {
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\simsun.ttc",
        "/usr/share/fonts/truetype/wqy/wqy-microhei.ttc",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    let mut fonts = FontDefinitions::default();
    for path in candidates {
        if let Ok(bytes) = std::fs::read(path) {
            let name = "cjk".to_string();
            fonts.font_data.insert(name.clone(), Arc::new(FontData::from_owned(bytes)));
            fonts.families.entry(FontFamily::Proportional).or_default().insert(0, name.clone());
            fonts.families.entry(FontFamily::Monospace).or_default().push(name.clone());
            ctx.set_fonts(fonts);
            ctx.all_styles_mut(|s| {
                s.text_styles.insert(TextStyle::Body, FontId::new(SIZE_BODY, FontFamily::Proportional));
                s.text_styles.insert(TextStyle::Button, FontId::new(SIZE_BODY, FontFamily::Proportional));
                s.text_styles.insert(TextStyle::Small, FontId::new(SIZE_SMALL, FontFamily::Proportional));
                s.text_styles.insert(TextStyle::Heading, FontId::new(SIZE_HEAD, FontFamily::Proportional));
                s.text_styles.insert(TextStyle::Monospace, FontId::new(SIZE_MONO, FontFamily::Monospace));
                s.spacing.item_spacing = egui::vec2(8.0, 7.0);
                s.spacing.interact_size.y = INPUT_H;
                s.spacing.button_padding = egui::vec2(10.0, 5.0);
            });
            return path.to_string();
        }
    }
    ctx.set_fonts(fonts);
    String::new()
}

/// 把配色应用到 egui 视觉
pub fn apply(ctx: &egui::Context, p: &Palette, dark_mode: bool) {
    ctx.set_visuals(if dark_mode { egui::Visuals::dark() } else { egui::Visuals::light() });
    let radius = egui::CornerRadius::same(RADIUS);
    ctx.all_styles_mut(move |s| {
        s.visuals.widgets.noninteractive.bg_fill = p.panel;
        s.visuals.widgets.noninteractive.fg_stroke.color = p.text;
        s.visuals.widgets.noninteractive.bg_stroke.color = p.border;
        // 描边宽度四态一致：悬停时按钮不会因为边框变粗/变细而"长大缩小"
        s.visuals.widgets.noninteractive.bg_stroke.width = 1.0;
        s.visuals.widgets.inactive.bg_stroke.width = 1.0;
        s.visuals.widgets.hovered.bg_stroke.width = 1.0;
        s.visuals.widgets.active.bg_stroke.width = 1.0;
        s.visuals.widgets.inactive.bg_fill = p.panel2;
        s.visuals.widgets.inactive.weak_bg_fill = p.panel2;
        s.visuals.widgets.inactive.fg_stroke.color = p.text;
        s.visuals.widgets.inactive.bg_stroke.color = p.border;
        s.visuals.widgets.hovered.bg_fill = p.brand.gamma_multiply(0.30);
        s.visuals.widgets.hovered.weak_bg_fill = p.brand.gamma_multiply(0.22);
        s.visuals.widgets.hovered.fg_stroke.color = p.text;
        s.visuals.widgets.hovered.bg_stroke.color = p.brand;
        s.visuals.widgets.active.bg_fill = p.brand_d;
        s.visuals.widgets.active.weak_bg_fill = p.brand_d;
        s.visuals.widgets.active.bg_stroke.color = p.brand;
        s.visuals.selection.bg_fill = p.brand.gamma_multiply(0.45);
        s.visuals.selection.stroke.color = p.text;
        s.visuals.panel_fill = p.bg;
        s.visuals.window_fill = p.panel;
        s.visuals.extreme_bg_color = p.bg;
        s.visuals.faint_bg_color = p.card;
        s.visuals.override_text_color = Some(p.text);
        for w in [
            &mut s.visuals.widgets.noninteractive,
            &mut s.visuals.widgets.inactive,
            &mut s.visuals.widgets.hovered,
            &mut s.visuals.widgets.active,
        ] {
            w.corner_radius = radius;
        }
    });
}

/// 正文 / 小字 / 标题便捷构造
pub fn body(text: impl Into<String>, p: &Palette) -> egui::RichText {
    egui::RichText::new(text).size(SIZE_BODY).color(p.text)
}
pub fn dim(text: impl Into<String>, p: &Palette) -> egui::RichText {
    egui::RichText::new(text).size(SIZE_SMALL).color(p.text2)
}
