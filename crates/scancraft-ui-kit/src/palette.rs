//! Ctrl+K command palette: fuzzy match over command labels, English labels
//! and stable ids. Pattern mirrors PdfCraft's `palette.rs` (see NOTICE).

use egui::{Align2, CornerRadius, Sense, vec2};

/// One palette row, built by the app from the engine command table.
pub struct Entry {
    pub id: &'static str,
    pub label: String,
    pub detail: String,
    pub ready: bool,
}

fn score(hay: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    let h = hay.to_lowercase();
    if let Some(p) = h.find(needle) {
        return Some(p);
    }
    // Subsequence match as a fallback.
    let mut it = h.chars();
    needle.chars().all(|c| it.any(|x| x == c)).then_some(100)
}

/// Draw the palette when `open`. Returns the chosen command id, if any.
pub fn show(ctx: &egui::Context, open: &mut bool, query: &mut String, entries: &[Entry]) -> Option<&'static str> {
    if !*open {
        return None;
    }
    if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        *open = false;
        query.clear();
        return None;
    }
    let q = query.trim().to_lowercase();
    let mut hits: Vec<(usize, &Entry)> = Vec::new();
    for e in entries {
        if let Some(s) = score(&e.label, &q).or_else(|| score(e.id, &q).map(|s| s + 50)) {
            hits.push((s, e));
        }
    }
    hits.sort_by_key(|(s, e)| (*s, !e.ready));
    hits.truncate(12);

    let mut chosen: Option<&'static str> = None;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("scancraft-palette"))
        .order(egui::Order::Foreground)
        .pivot(Align2::CENTER_TOP)
        .fixed_pos(egui::pos2(screen.center().x, screen.top() + 96.0))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).corner_radius(CornerRadius::same(12)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
                ui.set_width(560.0);
                ui.horizontal(|ui| {
                    ui.label("⌘K");
                    let r = ui.add(
                        egui::TextEdit::singleline(query)
                            .hint_text("Search tools and commands…")
                            .frame(egui::Frame::NONE)
                            .desired_width(f32::INFINITY),
                    );
                    if (r.has_focus() || r.lost_focus())
                        && ui.input(|i| i.key_pressed(egui::Key::Enter))
                        && let Some((_, h)) = hits.first()
                    {
                        chosen = Some(h.id);
                    }
                    r.request_focus();
                });
                ui.separator();
                for (_, h) in &hits {
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
                    let t = super::Tokens::get(ctx);
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, CornerRadius::same(6), t.hover);
                    }
                    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, h.ready, &h.label));
                    let fg = if h.ready { t.text } else { t.text_faint };
                    ui.painter().text(rect.left_center() + vec2(12.0, 0.0), Align2::LEFT_CENTER, &h.label, super::regular(13.5), fg);
                    ui.painter().text(rect.right_center() - vec2(10.0, 0.0), Align2::RIGHT_CENTER, &h.detail, super::regular(12.0), t.text_faint);
                    if resp.clicked() {
                        chosen = Some(h.id);
                    }
                }
                if hits.is_empty() {
                    ui.label("No matching tools or commands.");
                }
            });
        });
    if chosen.is_some() {
        *open = false;
        query.clear();
    }
    chosen
}
