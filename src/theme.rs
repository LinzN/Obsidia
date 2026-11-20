//! Fluent-like dark theme: a calm Mica-style background, layered surfaces with
//! hairline borders, small corner radii and a light blue accent.

use eframe::egui::{self, epaint, Color32, CornerRadius, Margin, Shadow, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(0x60, 0xcd, 0xff);
pub const GREEN: Color32 = Color32::from_rgb(0x6c, 0xcb, 0x5f);
pub const ORANGE: Color32 = Color32::from_rgb(0xfc, 0xe1, 0x00);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x99, 0xa4);

/// Fill for destructive actions (text on it is white).
pub const DANGER: Color32 = Color32::from_rgb(0xc4, 0x2b, 0x1c);

const TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const BASE: Color32 = Color32::from_rgb(0x20, 0x20, 0x20);
const LAYER: Color32 = Color32::from_rgb(0x2b, 0x2b, 0x2b);

pub fn apply(ctx: &egui::Context) {
    load_system_font(ctx);

    let mut v = egui::Visuals::dark();
    let control = CornerRadius::same(4);

    v.override_text_color = Some(TEXT);
    v.weak_text_color = Some(Color32::from_rgb(0xc5, 0xc5, 0xc5));
    v.panel_fill = Color32::TRANSPARENT;
    v.faint_bg_color = Color32::from_white_alpha(5);
    v.extreme_bg_color = Color32::from_rgb(0x1c, 0x1c, 0x1c);
    v.code_bg_color = Color32::from_rgb(0x1c, 0x1c, 0x1c);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = Color32::from_white_alpha(18);
    v.selection.stroke = Stroke::new(1.0, TEXT);

    v.window_fill = Color32::from_rgb(0x2c, 0x2c, 0x2c);
    v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(22));
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.window_shadow = Shadow {
        offset: [0, 8],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(16));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = control;

    for (state, fill, stroke) in [
        (&mut w.inactive, 15u8, 20u8),
        (&mut w.hovered, 22, 26),
        (&mut w.active, 10, 18),
        (&mut w.open, 22, 26),
    ] {
        state.bg_fill = Color32::from_white_alpha(fill);
        state.weak_bg_fill = Color32::from_white_alpha(fill);
        state.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(stroke));
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = control;
        state.expansion = 0.0;
    }

    ctx.set_visuals(v);
}

/// Inter (clear, screen-optimized UI font) for text, Phosphor as a fallback
/// for icons, and slightly larger default sizes so small text stays crisp.
fn load_system_font(ctx: &egui::Context) {
    use egui::{FontData, FontFamily, FontId, TextStyle};
    use std::sync::Arc;

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    let mut icons = FontData::from_static(crate::icons::FONT);
    // Phosphor glyphs sit a bit high next to text
    icons.tweak.y_offset_factor = 0.08;
    fonts.font_data.insert("phosphor".into(), Arc::new(icons));

    // Phosphor first: it only contains icons (private use area), and Inter has
    // its own glyphs at some of those code points.
    if let Some(f) = fonts.families.get_mut(&FontFamily::Proportional) {
        f.insert(0, "inter".into());
        f.insert(0, "phosphor".into());
    }
    if let Some(f) = fonts.families.get_mut(&FontFamily::Monospace) {
        f.insert(0, "phosphor".into());
    }
    fonts.families.insert(
        FontFamily::Name(SEMIBOLD.into()),
        vec!["phosphor".into(), "inter-semibold".into()],
    );
    ctx.set_fonts(fonts);

    ctx.all_styles_mut(|style| {
        style.text_styles = [
            (TextStyle::Small, FontId::proportional(11.5)),
            (TextStyle::Body, FontId::proportional(14.0)),
            (TextStyle::Button, FontId::proportional(14.0)),
            (
                TextStyle::Heading,
                FontId::new(20.0, FontFamily::Name(SEMIBOLD.into())),
            ),
            (TextStyle::Monospace, FontId::monospace(12.5)),
        ]
        .into();

        // consistent spacing for the larger text sizes
        let sp = &mut style.spacing;
        sp.item_spacing = egui::vec2(8.0, 6.0);
        sp.button_padding = egui::vec2(10.0, 5.0);
        sp.interact_size = egui::vec2(40.0, 28.0);
        sp.icon_width = 16.0;
        sp.icon_width_inner = 8.0;
        sp.icon_spacing = 6.0;
        sp.window_margin = egui::Margin::same(16);
        sp.menu_margin = egui::Margin::same(8);
        sp.combo_height = 320.0;
    });
}

/// Font family name for headings and titles.
pub const SEMIBOLD: &str = "semibold";

/// Semibold text, used for titles.
pub fn title(text: impl Into<String>, size: f32) -> egui::RichText {
    egui::RichText::new(text)
        .family(egui::FontFamily::Name(SEMIBOLD.into()))
        .size(size)
}

/// Mica-like window background: almost flat with a faint tint.
pub fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let mut mesh = epaint::Mesh::default();
    for (p, c) in [
        (rect.left_top(), Color32::from_rgb(0x22, 0x24, 0x27)),
        (rect.right_top(), Color32::from_rgb(0x20, 0x22, 0x25)),
        (rect.right_bottom(), BASE),
        (rect.left_bottom(), Color32::from_rgb(0x1f, 0x20, 0x22)),
    ] {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    ui.painter().add(mesh);
}

#[derive(Clone, Copy)]
pub enum Surface {
    /// Main content layer.
    Layer,
    /// Card inside a layer.
    Card,
    /// Dialogs and notices.
    Raised,
}

pub fn surface_frame(kind: Surface) -> egui::Frame {
    let (fill, stroke, margin, shadow) = match kind {
        Surface::Layer => (LAYER, Color32::from_white_alpha(12), 16, Shadow::NONE),
        Surface::Card => (
            Color32::from_white_alpha(8),
            Color32::from_white_alpha(14),
            12,
            Shadow::NONE,
        ),
        Surface::Raised => (
            Color32::from_rgb(0x2c, 0x2c, 0x2c),
            Color32::from_white_alpha(22),
            16,
            Shadow {
                offset: [0, 4],
                blur: 12,
                spread: 0,
                color: Color32::from_black_alpha(60),
            },
        ),
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(8)
        .inner_margin(Margin::same(margin))
        .shadow(shadow)
}

pub fn surface<R>(ui: &mut egui::Ui, kind: Surface, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    surface_frame(kind).show(ui, add).inner
}

/// Like [`surface`], but fills the whole available area.
pub fn surface_fill<R>(
    ui: &mut egui::Ui,
    kind: Surface,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    surface(ui, kind, |ui| {
        ui.set_min_size(ui.available_size());
        add(ui)
    })
}

/// Filled button. Light fills get dark text, dark fills white text.
pub fn accent_button(text: &str, color: Color32, enabled: bool) -> egui::Button<'static> {
    let (fill, text_color) = if enabled {
        let luminance =
            0.299 * color.r() as f32 + 0.587 * color.g() as f32 + 0.114 * color.b() as f32;
        let text = if luminance > 150.0 {
            Color32::BLACK
        } else {
            Color32::WHITE
        };
        (color, text)
    } else {
        (
            Color32::from_white_alpha(12),
            Color32::from_white_alpha(110),
        )
    };
    egui::Button::new(
        egui::RichText::new(text.to_string())
            .color(text_color)
            .strong(),
    )
    .fill(fill)
    .stroke(Stroke::new(
        1.0,
        Color32::from_white_alpha(if enabled { 30 } else { 12 }),
    ))
    .corner_radius(4)
}
