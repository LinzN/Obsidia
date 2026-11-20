//! Glass style for the GUI. egui has no backdrop blur, so the look is faked
//! with a soft gradient background and translucent, highlighted panels.

use eframe::egui::{self, epaint, Color32, CornerRadius, Margin, Pos2, Rect, Shadow, Stroke};

pub const ACCENT: Color32 = Color32::from_rgb(0x3d, 0x8b, 0xfd);
pub const GREEN: Color32 = Color32::from_rgb(0x3f, 0xc8, 0x6e);
pub const ORANGE: Color32 = Color32::from_rgb(0xf5, 0xa5, 0x24);
pub const RED: Color32 = Color32::from_rgb(0xef, 0x4f, 0x5a);

pub const RADIUS: u8 = 20;
const TEXT: Color32 = Color32::from_rgb(0xe9, 0xec, 0xf1);

/// Configure global egui visuals for the glass theme.
pub fn apply(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    let pill = CornerRadius::same(12);

    v.override_text_color = Some(TEXT);
    v.panel_fill = Color32::TRANSPARENT;
    v.faint_bg_color = Color32::from_white_alpha(8);
    v.extreme_bg_color = Color32::from_black_alpha(70);
    v.code_bg_color = Color32::from_black_alpha(60);
    v.hyperlink_color = ACCENT;
    v.selection.bg_fill = ACCENT.gamma_multiply(0.38);
    v.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    v.window_fill = Color32::from_rgba_unmultiplied(30, 33, 39, 240);
    v.window_stroke = Stroke::new(1.0, Color32::from_white_alpha(46));
    v.window_corner_radius = CornerRadius::same(RADIUS);
    v.menu_corner_radius = CornerRadius::same(14);
    v.window_shadow = Shadow {
        offset: [0, 12],
        blur: 40,
        spread: 0,
        color: Color32::from_black_alpha(110),
    };
    v.popup_shadow = Shadow {
        offset: [0, 8],
        blur: 24,
        spread: 0,
        color: Color32::from_black_alpha(90),
    };

    let w = &mut v.widgets;
    w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(26));
    w.noninteractive.fg_stroke = Stroke::new(1.0, TEXT);
    w.noninteractive.corner_radius = pill;

    for (state, fill, stroke) in [
        (&mut w.inactive, 20u8, 34u8),
        (&mut w.hovered, 38, 70),
        (&mut w.active, 58, 90),
        (&mut w.open, 38, 70),
    ] {
        state.bg_fill = Color32::from_white_alpha(fill);
        state.weak_bg_fill = Color32::from_white_alpha(fill);
        state.bg_stroke = Stroke::new(1.0, Color32::from_white_alpha(stroke));
        state.fg_stroke = Stroke::new(1.0, TEXT);
        state.corner_radius = pill;
    }
    w.hovered.expansion = 1.0;
    w.active.expansion = 1.0;

    ctx.set_visuals(v);
}

/// Paint the colorful backdrop behind all glass surfaces.
pub fn paint_backdrop(ui: &egui::Ui) {
    let rect = ui.max_rect();
    let painter = ui.painter();

    // Diagonal graphite gradient.
    let mut mesh = epaint::Mesh::default();
    let corners = [
        (rect.left_top(), Color32::from_rgb(0x1e, 0x22, 0x29)),
        (rect.right_top(), Color32::from_rgb(0x17, 0x1b, 0x22)),
        (rect.right_bottom(), Color32::from_rgb(0x0d, 0x0f, 0x13)),
        (rect.left_bottom(), Color32::from_rgb(0x12, 0x15, 0x1a)),
    ];
    for (p, c) in corners {
        mesh.colored_vertex(p, c);
    }
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);

    // Faint cool glows that the glass panels pick up - deliberately subtle.
    let size = rect.size();
    let blobs = [
        (0.85, 0.05, 0.45, Color32::from_rgb(0x3d, 0x8b, 0xfd), 46u8),
        (0.10, 0.95, 0.40, Color32::from_rgb(0x2a, 0xa6, 0xb8), 30),
        (0.35, 0.30, 0.35, Color32::from_rgb(0x8f, 0xa6, 0xc4), 18),
    ];
    for (x, y, r, color, alpha) in blobs {
        let center = rect.min + egui::vec2(size.x * x, size.y * y);
        radial_blob(painter, center, size.x.max(size.y) * r, color, alpha);
    }
}

fn radial_blob(painter: &egui::Painter, center: Pos2, radius: f32, color: Color32, alpha: u8) {
    const SEGMENTS: u32 = 64;
    let inner = Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha);
    let mut mesh = epaint::Mesh::default();
    mesh.colored_vertex(center, inner);
    for i in 0..SEGMENTS {
        let a = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(
            center + radius * egui::vec2(a.cos(), a.sin()),
            Color32::TRANSPARENT,
        );
    }
    for i in 0..SEGMENTS {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % SEGMENTS);
    }
    painter.add(mesh);
}

/// Glass surface strengths.
#[derive(Clone, Copy)]
pub enum Glass {
    /// Large floating panel.
    Panel,
    /// Card nested inside a panel.
    Card,
    /// Darker, calmer surface (log, dialogs).
    Tinted,
}

pub fn glass_frame(kind: Glass) -> egui::Frame {
    let (fill, stroke, radius, margin, shadow) = match kind {
        Glass::Panel => (
            Color32::from_white_alpha(10),
            Color32::from_white_alpha(32),
            RADIUS,
            14,
            Shadow {
                offset: [0, 10],
                blur: 30,
                spread: 0,
                color: Color32::from_black_alpha(70),
            },
        ),
        Glass::Card => (
            Color32::from_white_alpha(9),
            Color32::from_white_alpha(24),
            14,
            12,
            Shadow::NONE,
        ),
        Glass::Tinted => (
            Color32::from_rgba_unmultiplied(6, 7, 10, 120),
            Color32::from_white_alpha(36),
            RADIUS,
            12,
            Shadow {
                offset: [0, 10],
                blur: 30,
                spread: 0,
                color: Color32::from_black_alpha(70),
            },
        ),
    };
    egui::Frame::new()
        .fill(fill)
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(radius)
        .inner_margin(Margin::same(margin))
        .shadow(shadow)
}

/// Show `add` on a glass surface and paint its specular highlight.
pub fn glass<R>(ui: &mut egui::Ui, kind: Glass, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let radius = match kind {
        Glass::Card => 14.0,
        _ => RADIUS as f32,
    };
    let inner = glass_frame(kind).show(ui, add);
    paint_sheen(ui.painter(), inner.response.rect, radius);
    inner.inner
}

/// Like [`glass`], but stretches the surface over the whole available area.
pub fn glass_fill<R>(ui: &mut egui::Ui, kind: Glass, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    glass(ui, kind, |ui| {
        ui.set_min_size(ui.available_size());
        add(ui)
    })
}

/// Bright top edge plus a faint vertical sheen, mimicking light caught by glass.
fn paint_sheen(painter: &egui::Painter, rect: Rect, radius: f32) {
    let top = rect.top() + 0.75;
    painter.line_segment(
        [
            Pos2::new(rect.left() + radius, top),
            Pos2::new(rect.right() - radius, top),
        ],
        Stroke::new(1.0, Color32::from_white_alpha(60)),
    );

    let sheen = Rect::from_min_max(
        Pos2::new(rect.left() + 1.0, rect.top() + radius * 0.5),
        Pos2::new(
            rect.right() - 1.0,
            rect.top() + (rect.height() * 0.35).min(120.0),
        ),
    );
    if sheen.height() > 4.0 {
        let mut mesh = epaint::Mesh::default();
        let a = Color32::from_white_alpha(8);
        let b = Color32::TRANSPARENT;
        mesh.colored_vertex(sheen.left_top(), a);
        mesh.colored_vertex(sheen.right_top(), a);
        mesh.colored_vertex(sheen.right_bottom(), b);
        mesh.colored_vertex(sheen.left_bottom(), b);
        mesh.add_triangle(0, 1, 2);
        mesh.add_triangle(0, 2, 3);
        painter.add(mesh);
    }
}

/// Filled accent "pill" button.
pub fn accent_button(text: &str, color: Color32, enabled: bool) -> egui::Button<'static> {
    let fill = if enabled {
        color
    } else {
        Color32::from_white_alpha(18)
    };
    egui::Button::new(
        egui::RichText::new(text.to_string())
            .color(Color32::WHITE)
            .strong(),
    )
    .fill(fill)
    .stroke(Stroke::new(
        1.0,
        Color32::from_white_alpha(if enabled { 80 } else { 24 }),
    ))
    .corner_radius(14)
}
