// Everything on screen. The layout follows the web version:
//
//   top bar:  name · search · default folder · theme
//   left:     favorites, statistics
//   next:     all songs
//   middle:   the song that's playing
//   bottom:   the player bar
use crate::app::{App, Row, SORTS, format_time, plural};
use crate::analysis::BARS;
use crate::names::{self, UNKNOWN_ARTIST};
use crate::theme::{THEMES, Theme};
use eframe::egui::{
    self, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Frame, Layout, Margin, Mesh, Pos2, Rect, RichText, Sense, Shape,
    Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use egui_phosphor::regular as icon;
use std::sync::Arc;
use std::time::Duration;

const ROW: f32 = 46.0; // a line in "All songs"

// ---------- Fonts: Inter for text, Phosphor for icons ----------

pub fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let add = |fonts: &mut egui::FontDefinitions, name: &str, bytes: &'static [u8]| {
        fonts.font_data.insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
    };
    add(&mut fonts, "inter", include_bytes!("../assets/Inter-Regular.ttf"));
    add(&mut fonts, "inter-semibold", include_bytes!("../assets/Inter-SemiBold.ttf"));
    add(&mut fonts, "inter-bold", include_bytes!("../assets/Inter-Bold.ttf"));
    // Icons have families of their own: Inter has a few glyphs where the icons are, and the
    // icon font has invisible letters (so neither can come first for both)
    fonts.font_data.insert("phosphor".into(), Arc::new(egui_phosphor::Variant::Regular.font_data()));
    fonts.font_data.insert("phosphor-fill".into(), Arc::new(egui_phosphor::Variant::Fill.font_data()));
    let defaults = fonts.families[&FontFamily::Proportional].clone(); // egui's own fonts, for anything else
    for (family, text_font) in [(FontFamily::Proportional, "inter"), (FontFamily::Name("semibold".into()), "inter-semibold"), (FontFamily::Name("bold".into()), "inter-bold")] {
        let mut list = vec![text_font.to_string()];
        list.extend(defaults.iter().cloned());
        fonts.families.insert(family, list);
    }
    fonts.families.insert(FontFamily::Name("icons".into()), vec!["phosphor".into()]);
    fonts.families.insert(FontFamily::Name("fill".into()), vec!["phosphor-fill".into()]);
    ctx.set_fonts(fonts);
}

fn semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("semibold".into()))
}

fn bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("bold".into()))
}

fn regular(size: f32) -> FontId {
    FontId::proportional(size)
}

fn filled(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("fill".into()))
}

fn icons(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name("icons".into()))
}

// An icon followed by text, for buttons ("📁 Set default folder")
fn with_icon(glyph: &str, text: &str, font: FontId, color: Color32) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = |font_id: FontId| egui::TextFormat { font_id, color, valign: Align::Center, ..Default::default() };
    job.append(glyph, 0.0, format(icons(font.size + 2.0)));
    job.append(text, 7.0, format(font));
    job
}

// ---------- Theme: egui's own colors follow the chosen theme ----------

fn apply_theme(ctx: &egui::Context, t: &Theme) {
    ctx.style_mut(|style| {
        let mut v = if t.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.override_text_color = Some(t.text);
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = t.menu;
        v.window_stroke = Stroke::new(1.0, t.border);
        v.window_corner_radius = CornerRadius::same(14);
        v.menu_corner_radius = CornerRadius::same(12);
        v.extreme_bg_color = t.surface_strong;
        v.faint_bg_color = t.hover;
        v.hyperlink_color = t.accent;
        v.selection.bg_fill = t.accent_soft;
        v.selection.stroke = Stroke::new(1.0, t.accent);
        v.text_cursor.stroke = Stroke::new(2.0, t.accent);
        let shadow = egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if t.dark { 110 } else { 40 }) };
        v.window_shadow = shadow;
        v.popup_shadow = shadow;
        for widget in [&mut v.widgets.inactive, &mut v.widgets.hovered, &mut v.widgets.active, &mut v.widgets.open] {
            widget.corner_radius = CornerRadius::same(10);
            widget.fg_stroke = Stroke::new(1.0, t.text);
            widget.bg_stroke = Stroke::NONE;
        }
        v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
        v.widgets.inactive.bg_fill = t.hover;
        v.widgets.inactive.weak_bg_fill = t.hover;
        v.widgets.hovered.bg_fill = t.track;
        v.widgets.hovered.weak_bg_fill = t.track;
        v.widgets.active.bg_fill = t.accent_soft;
        v.widgets.active.weak_bg_fill = t.accent_soft;
        style.visuals = v;
        style.spacing.item_spacing = vec2(8.0, 6.0);
        style.spacing.button_padding = vec2(12.0, 6.0);
        style.spacing.scroll.bar_width = 6.0;
        style.spacing.scroll.floating = true;
        style.text_styles.insert(egui::TextStyle::Body, regular(14.0));
        style.text_styles.insert(egui::TextStyle::Button, regular(14.0));
        style.text_styles.insert(egui::TextStyle::Small, regular(12.0));
        style.text_styles.insert(egui::TextStyle::Heading, bold(24.0));
    });
}

// ---------- The whole screen ----------

pub fn draw(app: &mut App, ctx: &egui::Context) {
    // A theme being pointed at in the menu is shown right away (a preview)
    let theme: &'static Theme = app.hover_theme.map_or(app.theme, crate::theme::by_id);
    if app.styled_theme != Some(theme.id) {
        apply_theme(ctx, theme);
        app.styled_theme = Some(theme.id);
    }
    paint_background(app, ctx, theme);
    keyboard(app, ctx);

    egui::TopBottomPanel::top("top").show_separator_line(false).frame(Frame::NONE.inner_margin(Margin { left: 20, right: 20, top: 14, bottom: 8 })).show(ctx, |ui| top_bar(app, ui, theme));
    egui::TopBottomPanel::bottom("bar").show_separator_line(false).frame(Frame::NONE.inner_margin(Margin { left: 16, right: 16, top: 6, bottom: 14 })).show(ctx, |ui| player_bar(app, ui, theme));
    egui::SidePanel::left("left").resizable(false).show_separator_line(false).exact_width(270.0).frame(Frame::NONE.inner_margin(Margin { left: 16, right: 6, top: 6, bottom: 6 })).show(ctx, |ui| {
        let height = ui.available_height();
        let favorites_height = (height * 0.5).max(160.0);
        ui.allocate_ui(vec2(ui.available_width(), favorites_height), |ui| favorites_card(app, ui, theme));
        ui.add_space(10.0);
        stats_card(app, ui, theme);
    });
    egui::SidePanel::left("songs").resizable(false).show_separator_line(false).exact_width(340.0).frame(Frame::NONE.inner_margin(Margin { left: 6, right: 6, top: 6, bottom: 6 })).show(ctx, |ui| songs_card(app, ui, theme));
    egui::SidePanel::right("lyrics").resizable(false).show_separator_line(false).exact_width(330.0).frame(Frame::NONE.inner_margin(Margin { left: 6, right: 16, top: 6, bottom: 6 })).show(ctx, |ui| lyrics_card(app, ui, theme));
    egui::CentralPanel::default().frame(Frame::NONE.inner_margin(Margin { left: 12, right: 12, top: 6, bottom: 6 })).show(ctx, |ui| {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("middle-rect"), ui.max_rect()));
        now_playing(app, ui, theme)
    });

    search_results(app, ctx, theme);
    lyrics_menu(app, ctx, theme);
    edit_window(app, ctx, theme);
    tidy_window(app, ctx, theme);
    delete_window(app, ctx, theme);
    duplicates_window(app, ctx, theme);
    wiki_window(app, ctx, theme);
    download_window(app, ctx, theme);
    import_window(app, ctx, theme);
    theme_menu(app, ctx, theme);
    toast(app, ctx, theme);
    welcome(app, ctx, theme);
}

// The background color with a soft glow of the theme's two accents (as in the web version)
fn paint_background(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let painter = ctx.layer_painter(egui::LayerId::background());
    let screen = ctx.content_rect();
    painter.rect_filled(screen, 0.0, t.bg);
    // The playing song's cover as a soft wash of color, fading over the previous one
    let strength = if t.dark { 0.34 } else { 0.24 };
    let aspect = screen.width() / screen.height().max(1.0);
    let uv = if aspect > 1.0 {
        Rect::from_min_max(pos2(0.0, 0.5 - 0.5 / aspect), pos2(1.0, 0.5 + 0.5 / aspect))
    } else {
        Rect::from_min_max(pos2(0.5 - 0.5 * aspect, 0.0), pos2(0.5 + 0.5 * aspect, 1.0))
    };
    let fade = |since: std::time::Instant| (since.elapsed().as_secs_f32() / 0.8).min(1.0);
    let mut new_alpha = 1.0;
    if let Some((old, since)) = &app.old_backdrop {
        let t = fade(*since);
        painter.image(old.id(), screen, uv, Color32::WHITE.gamma_multiply(strength * (1.0 - t)));
        new_alpha = t;
        if t >= 1.0 {
            app.old_backdrop = None;
        }
    }
    if let Some((_, texture)) = &app.backdrop {
        if app.current.is_some() {
            painter.image(texture.id(), screen, uv, Color32::WHITE.gamma_multiply(strength * new_alpha));
        }
    }
    let glow = |painter: &egui::Painter, center: Pos2, radius: Vec2, color: Color32| {
        let mut mesh = Mesh::default();
        let middle = mesh.vertices.len() as u32;
        mesh.colored_vertex(center, color.gamma_multiply(t.glow));
        let steps = 64;
        for i in 0..=steps {
            let angle = i as f32 / steps as f32 * std::f32::consts::TAU;
            mesh.colored_vertex(center + vec2(angle.cos() * radius.x, angle.sin() * radius.y), Color32::TRANSPARENT);
            if i > 0 {
                mesh.add_triangle(middle, middle + i, middle + i + 1);
            }
        }
        painter.add(Shape::mesh(mesh));
    };
    glow(&painter, pos2(screen.width() * 0.08, 0.0), vec2(screen.width() * 0.55, screen.height() * 0.6), t.accent);
    glow(&painter, pos2(screen.width() * 0.92, screen.height()), vec2(screen.width() * 0.6, screen.height() * 0.55), t.accent2);
}

fn keyboard(app: &mut App, ctx: &egui::Context) {
    // No shortcuts while you type, or while a window is open (Space belongs to its fields)
    let window_open = app.editing.is_some() || app.tidy.is_some() || app.deleting.is_some() || app.duplicates.is_some() || app.welcome.is_some() || app.wiki_open || app.import_pending.is_some() || app.downloads.open;
    let typing = ctx.wants_keyboard_input() || ctx.memory(|m| m.focused().is_some()) || window_open;
    ctx.input(|input| {
        for event in &input.events {
            if let egui::Event::Key { key, pressed: true, repeat: false, .. } = event {
                match key {
                    egui::Key::Space if !typing && app.lyrics.as_ref().is_some_and(|v| v.tapping.is_some()) => app.tap(None),
                    egui::Key::Space if !typing => app.toggle_play(),
                    egui::Key::Slash if !typing => app.focus_search = true,
                    egui::Key::G if !typing => app.move_lyrics(-0.1), // the VLC keys for subtitles
                    egui::Key::H if !typing => app.move_lyrics(0.1),
                    egui::Key::Escape => {
                        app.sort_open = false;
                        app.theme_open = false;
                        app.hover_theme = None;
                        if typing && !window_open {
                            app.search.clear();
                        }
                    }
                    _ => {}
                }
            }
        }
    });
}

// A rounded panel with the theme's see-through surface color
fn card<R>(ui: &mut Ui, t: &Theme, add: impl FnOnce(&mut Ui) -> R) -> R {
    Frame::NONE
        .fill(t.surface)
        .corner_radius(CornerRadius::same(18))
        .inner_margin(Margin::same(14))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            add(ui)
        })
        .inner
}

fn heading(ui: &mut Ui, t: &Theme, text: &str) {
    ui.label(RichText::new(text.to_uppercase()).font(semibold(11.5)).color(t.muted).extra_letter_spacing(1.2));
}

// A round icon button that lights up when pointed at
fn icon_button(ui: &mut Ui, t: &Theme, glyph: &str, size: f32, color: Color32, tip: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    if response.hovered() {
        ui.painter().circle_filled(rect.center(), size / 2.0, t.hover);
    }
    let color = if response.hovered() && color == t.muted { t.text } else { color };
    ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, icons(size * 0.52), color);
    response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand)
}

// A picture with rounded corners, or a placeholder note when there's none (yet)
fn cover(ui: &mut Ui, t: &Theme, texture: Option<&egui::TextureHandle>, size: f32, radius: u8) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint_cover(ui.painter(), t, texture, rect, radius);
    response
}

fn paint_cover(painter: &egui::Painter, t: &Theme, texture: Option<&egui::TextureHandle>, rect: Rect, radius: u8) {
    match texture {
        Some(texture) => {
            let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
            painter.add(egui::epaint::RectShape::filled(rect, radius, Color32::WHITE).with_texture(texture.id(), uv));
        }
        None => {
            painter.rect_filled(rect, radius, t.accent_soft);
            painter.text(rect.center(), Align2::CENTER_CENTER, icon::MUSIC_NOTE, icons(rect.width() * 0.42), t.accent);
        }
    }
}

// Three small bars next to the song that's playing (moving while it plays)
fn now_bars(painter: &egui::Painter, center: Pos2, color: Color32, moving: bool, time: f64) {
    for i in 0..3 {
        let phase = time as f32 * 6.0 + i as f32 * 1.7;
        let height = if moving { 4.0 + 6.0 * (0.5 + 0.5 * phase.sin()) } else { [6.0, 10.0, 7.0][i] };
        let x = center.x - 5.0 + i as f32 * 5.0;
        painter.rect_filled(Rect::from_min_max(pos2(x - 1.5, center.y + 5.0 - height), pos2(x + 1.5, center.y + 5.0)), 1.0, color);
    }
}

// A slider drawn like the web version's: a thin track, filled in the accent colors
fn slider(ui: &mut Ui, t: &Theme, value: f32, max: f32, width: f32) -> Option<f32> {
    let (rect, response) = ui.allocate_exact_size(vec2(width, 18.0), Sense::click_and_drag());
    let track = Rect::from_center_size(rect.center(), vec2(rect.width(), 5.0));
    let fraction = if max > 0.0 { (value / max).clamp(0.0, 1.0) } else { 0.0 };
    let painter = ui.painter();
    painter.rect_filled(track, 3.0, t.track);
    let filled = Rect::from_min_max(track.min, pos2(track.min.x + track.width() * fraction, track.max.y));
    gradient_rect(painter, filled, t.accent, t.accent2);
    if response.hovered() || response.dragged() {
        painter.circle_filled(pos2(filled.max.x, track.center().y), 7.0, t.accent);
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if (response.dragged() || response.clicked()) && max > 0.0 {
        if let Some(pointer) = response.interact_pointer_pos() {
            return Some(((pointer.x - track.min.x) / track.width()).clamp(0.0, 1.0) * max);
        }
    }
    None
}

fn gradient_rect(painter: &egui::Painter, rect: Rect, left: Color32, right: Color32) {
    if rect.width() <= 0.0 {
        return;
    }
    // Rounded ends: a circle at each end, a gradient between them
    let r = rect.height() / 2.0;
    painter.circle_filled(pos2(rect.min.x + r, rect.center().y), r, left);
    painter.circle_filled(pos2(rect.max.x - r, rect.center().y), r, right);
    let inner = Rect::from_min_max(pos2(rect.min.x + r, rect.min.y), pos2((rect.max.x - r).max(rect.min.x + r), rect.max.y));
    let mut mesh = Mesh::default();
    mesh.colored_vertex(inner.left_top(), left);
    mesh.colored_vertex(inner.right_top(), right);
    mesh.colored_vertex(inner.right_bottom(), right);
    mesh.colored_vertex(inner.left_bottom(), left);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(Shape::mesh(mesh));
}

// ---------- Top bar ----------

fn top_bar(app: &mut App, ui: &mut Ui, t: &Theme) {
    ui.horizontal(|ui| {
        ui.set_height(40.0);
        ui.label(RichText::new(icon::MUSIC_NOTE).font(icons(22.0)).color(t.accent));
        ui.label(RichText::new("Music Player").font(bold(17.0)));
        ui.add_space(10.0);
        // Download: shows how it's going while songs download
        let active = app.downloads.active();
        let label = if active > 0 { format!("Downloading {active}…") } else { "Download".to_string() };
        let button = egui::Button::new(with_icon(icon::DOWNLOAD_SIMPLE, &label, semibold(13.5), if active > 0 { t.accent } else { t.text }))
            .fill(t.surface)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(18))
            .min_size(vec2(0.0, 36.0));
        if ui.add(button).on_hover_text("Download songs or playlists from YouTube and other sites").clicked() {
            app.open_downloads();
        }
        // Tidy file names and Wikipedia (just the icons in a narrower window)
        let wide = ui.ctx().content_rect().width() > 1500.0;
        let pill = |glyph: &str, text: &str| egui::Button::new(with_icon(glyph, if wide { text } else { "" }, semibold(13.5), t.text))
            .fill(t.surface).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(18)).min_size(vec2(36.0, 36.0));
        let tip = if app.busy { "Changing files…" } else { "Tidy file names: every file named \"Artist - Title\"" };
        if ui.add(pill(icon::MAGIC_WAND, "Tidy names")).on_hover_text(tip).clicked() && !app.busy {
            app.open_tidy();
        }
        let tip = if !app.wiki_list.is_empty() {
            format!("Albums and genres from Wikipedia: {} to look at", plural(app.wiki_list.len(), "song"))
        } else if app.wiki_waiting > 0 {
            format!("Albums and genres from Wikipedia (looking up {} more)", plural(app.wiki_waiting, "song"))
        } else {
            "Albums and genres from Wikipedia".to_string()
        };
        let disc = ui.add(pill(icon::DISC, "Albums & genres")).on_hover_text(tip);
        if !app.wiki_list.is_empty() {
            ui.painter().circle_filled(disc.rect.right_top() + vec2(-5.0, 5.0), 4.0, t.accent);
        }
        if disc.clicked() {
            app.refresh_wiki_list();
            app.wiki_open = true;
        }

        // The search box, centered over the album art (the middle column, as it was last frame)
        let middle: Rect = ui.ctx().data(|d| d.get_temp(egui::Id::new("middle-rect"))).unwrap_or(ui.max_rect());
        let width = (middle.width() - 48.0).clamp(240.0, 520.0);
        let spot = Rect::from_center_size(pos2(middle.center().x, ui.max_rect().center().y), vec2(width + 30.0, 40.0));
        let mut place = ui.new_child(egui::UiBuilder::new().max_rect(spot).layout(Layout::left_to_right(Align::Center)));
        {
        let ui = &mut place;
        let search = Frame::NONE
            .fill(t.surface_strong)
            .stroke(Stroke::new(1.0, t.border))
            .corner_radius(CornerRadius::same(20))
            .inner_margin(Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.label(RichText::new(icon::MAGNIFYING_GLASS).font(icons(16.0)).color(t.muted));
                    let edit = egui::TextEdit::singleline(&mut app.search)
                        .hint_text(RichText::new("Search songs, artists, albums").color(t.muted))
                        .frame(false)
                        .desired_width(f32::INFINITY)
                        .font(regular(14.5));
                    let response = ui.add(edit);
                    if app.focus_search {
                        response.request_focus();
                        app.focus_search = false;
                    }
                    response
                })
                .inner
            });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("search-rect"), search.response.rect));
        let focused = search.inner.has_focus(); // (asked before data_mut: egui's memory is locked inside it)
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("search-focused"), focused));
        }

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            // The theme menu: a swatch of the chosen theme
            let (rect, response) = ui.allocate_exact_size(vec2(56.0, 36.0), Sense::click());
            let painter = ui.painter();
            painter.rect(rect, 18.0, if response.hovered() { t.surface_strong } else { t.surface }, Stroke::new(1.0, t.border), StrokeKind::Inside);
            swatch(painter, pos2(rect.min.x + 18.0, rect.center().y), app.theme);
            painter.text(pos2(rect.max.x - 16.0, rect.center().y), Align2::CENTER_CENTER, if app.theme_open { icon::CARET_UP } else { icon::CARET_DOWN }, icons(13.0), t.muted);
            if response.on_hover_text("Theme").clicked() {
                app.theme_open = !app.theme_open;
                app.hover_theme = None;
            }
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("theme-button"), rect));
            ui.add_space(4.0);
            let folder = egui::Button::new(with_icon(icon::FOLDER_SIMPLE, "Set default folder", semibold(13.5), t.text))
                .fill(t.surface)
                .stroke(Stroke::new(1.0, t.border))
                .corner_radius(CornerRadius::same(18))
                .min_size(vec2(0.0, 36.0));
            let tip = format!("Default folder: {}\nClick to pick another one.", app.folder.display());
            if ui.add(folder).on_hover_text(tip).clicked() {
                app.choose_default_folder();
            }
            // Export and import: the whole library as one .zip (not the music files)
            // (just the icons in a narrower window, so the search box keeps its room)
            let wide = ui.ctx().content_rect().width() > 1560.0;
            let pill = |text: &str, glyph: &str| {
                let content = if wide { with_icon(glyph, text, semibold(13.5), t.text) } else { with_icon(glyph, "", semibold(13.5), t.text) };
                egui::Button::new(content).fill(t.surface).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(18)).min_size(vec2(36.0, 36.0))
            };
            if ui.add(pill("Export", icon::TRAY_ARROW_UP)).on_hover_text("Export: save your library (names, albums, genres, hearts, plays, covers, lyrics) as a .zip, to move it to another computer").clicked() {
                app.export_library();
            }
            if ui.add(pill("Import", icon::TRAY_ARROW_DOWN)).on_hover_text("Import: bring a library in from a .zip made with Export").clicked() {
                app.choose_import();
            }
        });
    });
}

// A theme's swatch: its background with its two accents in the middle
fn swatch(painter: &egui::Painter, center: Pos2, theme: &Theme) {
    painter.circle(center, 11.0, theme.bg, Stroke::new(1.0, theme.track));
    painter.circle_filled(center, 7.0, theme.accent);
    painter.circle_filled(center + vec2(2.5, 2.5), 4.0, theme.accent2);
}

// t: the theme on screen right now (the one pointed at, while previewing)
fn theme_menu(app: &mut App, ctx: &egui::Context, t: &Theme) {
    if !app.theme_open {
        return;
    }
    let button: Rect = ctx.data(|d| d.get_temp(egui::Id::new("theme-button"))).unwrap_or(Rect::NOTHING);
    let mut hovered = None;
    let area = egui::Area::new(egui::Id::new("theme-menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(button.max.x, button.max.y + 8.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(Margin::same(10)).show(ui, |ui| {
                ui.horizontal_top(|ui| {
                    for dark in [false, true] {
                        ui.vertical(|ui| {
                            ui.set_width(170.0);
                            heading(ui, t, if dark { "Dark" } else { "Light" });
                            for theme in THEMES.iter().filter(|theme| theme.dark == dark) {
                                let (rect, response) = ui.allocate_exact_size(vec2(170.0, 36.0), Sense::click());
                                if response.hovered() {
                                    ui.painter().rect_filled(rect, 10.0, t.hover);
                                    hovered = Some(theme.id);
                                }
                                swatch(ui.painter(), pos2(rect.min.x + 20.0, rect.center().y), theme);
                                let chosen = app.prefs.theme == theme.id;
                                ui.painter().text(pos2(rect.min.x + 42.0, rect.center().y), Align2::LEFT_CENTER, theme.name, if chosen { semibold(14.0) } else { regular(14.0) }, t.text);
                                if chosen {
                                    ui.painter().text(pos2(rect.max.x - 14.0, rect.center().y), Align2::CENTER_CENTER, icon::CHECK, icons(15.0), t.accent);
                                }
                                if response.clicked() {
                                    app.set_theme(theme.id);
                                    app.theme_open = false;
                                }
                            }
                        });
                    }
                });
            });
        });
    app.hover_theme = if app.theme_open { hovered } else { None };
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| button.contains(p))) {
        app.theme_open = false;
        app.hover_theme = None;
    }
}

fn search_results(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let focused: bool = ctx.data(|d| d.get_temp(egui::Id::new("search-focused"))).unwrap_or(false);
    let open_id = egui::Id::new("search-open");
    let was_open: bool = ctx.data(|d| d.get_temp(open_id)).unwrap_or(false);
    let open = !app.search.trim().is_empty() && (focused || was_open);
    if !open {
        ctx.data_mut(|d| d.insert_temp(open_id, false));
        return;
    }
    let rect: Rect = ctx.data(|d| d.get_temp(egui::Id::new("search-rect"))).unwrap_or(Rect::NOTHING);
    let mut play = None;
    let area = egui::Area::new(egui::Id::new("search-results"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos2(rect.min.x, rect.max.y + 6.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(Margin::same(6)).show(ui, |ui| {
                ui.set_width(rect.width() - 12.0);
                if app.search_results.is_empty() {
                    ui.add_space(6.0);
                    ui.label(RichText::new("No songs match.").color(t.muted));
                    ui.add_space(6.0);
                    return;
                }
                let height = (app.search_results.len() as f32 * (ROW + ui.spacing().item_spacing.y)).min(420.0);
                egui::ScrollArea::vertical().max_height(420.0).min_scrolled_height(height).show(ui, |ui| {
                    for &i in &app.search_results {
                        let song = &app.songs[i];
                        let playing = app.current.as_deref() == Some(song.id.as_str());
                        if song_line(ui, t, &song.title, &song.artist, playing, false, app.player.is_playing()).clicked() {
                            play = Some(song.id.clone());
                        }
                    }
                });
            });
        });
    let still_open = !area.response.clicked_elsewhere() || focused;
    ctx.data_mut(|d| d.insert_temp(open_id, still_open && play.is_none()));
    if let Some(id) = play {
        let list: Vec<String> = app.search_results.iter().map(|&i| app.songs[i].id.clone()).collect();
        app.play_from_list(&id, list);
        ctx.memory_mut(|m| m.surrender_focus(egui::Id::NULL));
    }
}

// A song as one line: title over artist (a list row, a search result)
fn song_line(ui: &mut Ui, t: &Theme, title: &str, artist: &str, playing: bool, active: bool, moving: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let painter = ui.painter();
    if playing || active {
        painter.rect_filled(rect, 12.0, t.accent_soft);
    } else if response.hovered() {
        painter.rect_filled(rect, 12.0, t.hover);
    }
    let mut x = rect.min.x + 12.0;
    if playing {
        now_bars(painter, pos2(x + 5.0, rect.min.y + 14.0), t.accent, moving, ui.input(|i| i.time));
        x += 16.0;
    }
    let title_rect = Rect::from_min_max(pos2(x, rect.min.y + 6.0), pos2(rect.max.x - 36.0, rect.min.y + 24.0));
    let artist_rect = Rect::from_min_max(pos2(rect.min.x + 12.0, rect.min.y + 24.0), pos2(rect.max.x - 36.0, rect.max.y - 4.0));
    clipped_text(painter, title_rect, title, semibold(14.0), if playing { t.accent } else { t.text });
    clipped_text(painter, artist_rect, artist, regular(12.5), t.muted);
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

// One line of text, cut with "…" when it doesn't fit
fn clipped_text(painter: &egui::Painter, rect: Rect, text: &str, font: FontId, color: Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(rect.width());
    let galley = painter.ctx().fonts_mut(|f| f.layout_job(job));
    painter.galley(pos2(rect.min.x, rect.center().y - galley.size().y / 2.0), galley, color);
}

// ---------- Favorites ----------

fn favorites_card(app: &mut App, ui: &mut Ui, t: &Theme) {
    card(ui, t, |ui| {
        let favorites = app.favorites();
        ui.horizontal(|ui| {
            heading(ui, t, "Favorites");
            if !favorites.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(favorites.len().to_string()).font(semibold(12.0)).color(t.muted));
                });
            }
        });
        ui.add_space(6.0);
        if favorites.is_empty() {
            ui.label(RichText::new("No favorites yet. Click the heart next to a song to keep it here.").color(t.muted));
            return;
        }
        let mut play = None;
        egui::ScrollArea::vertical().id_salt("favorites").auto_shrink([false, false]).show(ui, |ui| {
            for (id, title, artist) in &favorites {
                let here = app.song(id).is_some();
                if here {
                    app.request_cover(id, false);
                }
                let playing = app.current.as_deref() == Some(id.as_str());
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 50.0), Sense::click());
                let painter = ui.painter();
                if playing {
                    painter.rect_filled(rect, 12.0, t.accent_soft);
                } else if response.hovered() {
                    painter.rect_filled(rect, 12.0, t.hover);
                }
                let avatar = Rect::from_min_size(pos2(rect.min.x + 6.0, rect.min.y + 7.0), Vec2::splat(36.0));
                paint_cover(painter, t, app.textures.get(id).and_then(|c| c.small.as_ref()), avatar, 9);
                let text_left = avatar.max.x + 10.0;
                let color = if !here { t.muted } else if playing { t.accent } else { t.text };
                clipped_text(painter, Rect::from_min_max(pos2(text_left, rect.min.y + 7.0), pos2(rect.max.x - 8.0, rect.min.y + 25.0)), title, semibold(13.5), color);
                clipped_text(painter, Rect::from_min_max(pos2(text_left, rect.min.y + 25.0), pos2(rect.max.x - 8.0, rect.max.y - 7.0)), artist, regular(12.0), t.muted);
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                let response = if here { response } else { response.on_hover_text("Not in the open folder") };
                if response.clicked() {
                    play = Some(id.clone());
                }
            }
        });
        if let Some(id) = play {
            app.play_favorite(&id);
        }
    });
}

// ---------- Statistics ----------

fn stats_card(app: &mut App, ui: &mut Ui, t: &Theme) {
    card(ui, t, |ui| {
        let plays: u64 = app.library.songs.values().map(|r| r.plays).sum();
        ui.horizontal(|ui| {
            heading(ui, t, "Statistics");
            if plays > 0 {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(plural(plays as usize, "play")).font(regular(12.0)).color(t.muted));
                });
            }
        });
        ui.add_space(6.0);
        let by_genre = app.prefs.stats_genres;
        let known = |bars: &[crate::app::Bar]| bars.iter().filter(|bar| !bar.key.is_empty()).count();
        let middle = if by_genre { (known(&app.stats.genres), "genres") } else { (known(&app.stats.artists), "artists") };
        ui.columns(3, |columns| {
            for (column, (number, label)) in columns.iter_mut().zip([(app.stats.songs, "songs"), middle, (app.stats.albums, "albums")]) {
                Frame::NONE.fill(t.hover).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(10, 8)).show(column, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(RichText::new(number.to_string()).font(bold(20.0)));
                    ui.label(RichText::new(label).font(regular(11.5)).color(t.muted));
                });
            }
        });
        ui.add_space(8.0);

        // Artists or genres
        ui.horizontal(|ui| {
            Frame::NONE.fill(t.hover).corner_radius(CornerRadius::same(16)).inner_margin(Margin::same(3)).show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (genres, label) in [(false, "Artists"), (true, "Genres")] {
                    let on = by_genre == genres;
                    let button = egui::Button::new(RichText::new(label).font(semibold(12.0)).color(if on { t.accent } else { t.muted }))
                        .fill(if on { t.surface_strong } else { Color32::TRANSPARENT })
                        .corner_radius(CornerRadius::same(13))
                        .min_size(vec2(64.0, 24.0));
                    if ui.add(button).clicked() {
                        app.prefs.stats_genres = genres;
                    }
                }
            });
        });
        ui.add_space(4.0);

        let bars = if by_genre { &app.stats.genres } else { &app.stats.artists };
        if bars.is_empty() {
            ui.label(RichText::new(if app.songs.is_empty() { "Open a folder to see its statistics." } else { "No genres yet." }).color(t.muted));
            return;
        }
        let most = bars.iter().map(|bar| bar.songs).max().unwrap_or(1).max(1) as f32;
        let total = app.stats.songs.max(1) as f32;
        let current = app.current_song();
        let playing_artist = current.map(|s| if s.known_artist() { names::normalize(&s.artist) } else { String::new() });
        let playing_genres: Option<Vec<String>> = current.map(|s| names::genres_of(&s.genre).iter().map(|g| names::normalize(g)).collect());
        let active = if by_genre { app.genre_filter.clone() } else { app.artist_filter.clone() };
        let moving = app.player.is_playing();
        let mut clicked = None;
        egui::ScrollArea::vertical().id_salt("bars").auto_shrink([false, false]).show(ui, |ui| {
            for bar in bars {
                let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
                let painter = ui.painter();
                let is_active = active.as_deref() == Some(bar.key.as_str());
                if is_active {
                    painter.rect_filled(rect, 10.0, t.accent_soft);
                } else if response.hovered() {
                    painter.rect_filled(rect, 10.0, t.hover);
                }
                let is_playing = if by_genre {
                    playing_genres.as_ref().is_some_and(|genres| if bar.key.is_empty() { genres.is_empty() } else { genres.contains(&bar.key) })
                } else {
                    playing_artist.as_deref() == Some(bar.key.as_str())
                };
                let mut x = rect.min.x + 8.0;
                if is_playing {
                    now_bars(painter, pos2(x + 5.0, rect.min.y + 13.0), t.accent, moving, ui.input(|i| i.time));
                    x += 16.0;
                }
                let name_color = if bar.key.is_empty() { t.muted } else { t.text };
                clipped_text(painter, Rect::from_min_max(pos2(x, rect.min.y + 4.0), pos2(rect.max.x - 40.0, rect.min.y + 22.0)), &bar.name, semibold(13.0), name_color);
                painter.text(pos2(rect.max.x - 8.0, rect.min.y + 13.0), Align2::RIGHT_CENTER, bar.songs.to_string(), regular(12.0), t.muted);
                let track = Rect::from_min_max(pos2(rect.min.x + 8.0, rect.min.y + 26.0), pos2(rect.max.x - 8.0, rect.min.y + 30.0));
                painter.rect_filled(track, 2.0, t.track);
                gradient_rect(painter, Rect::from_min_max(track.min, pos2(track.min.x + track.width() * bar.songs as f32 / most, track.max.y)), t.accent, t.accent2);
                let share = (bar.songs as f32 / total * 100.0).round();
                let tip = format!("{}: {} ({share}%)", bar.name, plural(bar.songs, "song"));
                if response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    clicked = Some(bar.key.clone());
                }
            }
        });
        if let Some(key) = clicked {
            if by_genre { app.filter_by_genre(&key) } else { app.filter_by_artist(&key) }
        }
    });
}

// ---------- All songs ----------

fn songs_card(app: &mut App, ui: &mut Ui, t: &Theme) {
    card(ui, t, |ui| {
        ui.horizontal(|ui| {
            heading(ui, t, "All songs");
            if !app.songs.is_empty() {
                ui.label(RichText::new(app.songs.len().to_string()).font(semibold(12.0)).color(t.muted));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let short = SORTS.iter().find(|(id, _)| *id == app.prefs.sort).map_or("Folder", |(_, label)| label);
                let button = egui::Button::new(with_icon(icon::SORT_ASCENDING, short, semibold(12.0), t.muted))
                    .fill(if app.sort_open { t.hover } else { Color32::TRANSPARENT })
                    .corner_radius(CornerRadius::same(14))
                    .min_size(vec2(0.0, 28.0));
                let response = ui.add(button).on_hover_text("Sort and filter");
                if response.clicked() {
                    app.sort_open = !app.sort_open;
                }
                if app.sort_open {
                    sort_menu(app, ui.ctx(), response.rect, t);
                }

            });
        });
        let folder_name = app.folder.file_name().map_or_else(|| app.folder.display().to_string(), |n| n.to_string_lossy().to_string());
        ui.label(RichText::new(&folder_name).font(regular(12.0)).color(t.muted)).on_hover_text(app.folder.display().to_string());

        // Filters that are on, each with a way to turn it off
        let mut clear_artist = false;
        let mut clear_genre = false;
        let mut clear_favorites = false;
        if app.artist_filter.is_some() || app.genre_filter.is_some() || app.prefs.only_favorites {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                let chip = |ui: &mut Ui, text: String| {
                    let button = egui::Button::new({
                        let mut job = egui::text::LayoutJob::default();
                        let format = |font_id| egui::TextFormat { font_id, color: t.accent, valign: Align::Center, ..Default::default() };
                        job.append(&text, 0.0, format(semibold(12.0)));
                        job.append(icon::X, 6.0, format(icons(13.0)));
                        job
                    })
                        .fill(t.accent_soft)
                        .corner_radius(CornerRadius::same(12));
                    ui.add(button).on_hover_text("Show all again").clicked()
                };
                if let Some(key) = &app.artist_filter {
                    let name = if key.is_empty() { UNKNOWN_ARTIST.to_string() } else {
                        app.stats.artists.iter().find(|b| &b.key == key).map_or(key.clone(), |b| b.name.clone())
                    };
                    clear_artist = chip(ui, name);
                }
                if let Some(key) = &app.genre_filter {
                    let name = app.stats.genres.iter().find(|b| &b.key == key).map_or(key.clone(), |b| b.name.clone());
                    clear_genre = chip(ui, name);
                }
                if app.prefs.only_favorites {
                    clear_favorites = chip(ui, "Favorites".into());
                }
            });
        }
        if clear_artist {
            app.clear_filters(true, false);
        }
        if clear_genre {
            app.clear_filters(false, true);
        }
        if clear_favorites {
            app.set_only_favorites(false);
        }
        ui.add_space(6.0);

        if app.songs.is_empty() {
            let message = if !app.listed {
                "Reading the folder…".to_string()
            } else {
                format!("The {folder_name} folder has no songs yet. Copy some into it and they show up here by themselves.")
            };
            ui.label(RichText::new(message).color(t.muted));
            return;
        }
        if app.rows.is_empty() {
            ui.label(RichText::new("No songs match these filters.").color(t.muted));
            return;
        }

        // Scrolls just enough to show the song that starts playing
        let mut scroll = egui::ScrollArea::vertical().id_salt("songs").auto_shrink([false, false]);
        if app.scroll_to_current {
            app.scroll_to_current = false;
            if let Some(row) = app.current.as_ref().and_then(|id| app.rows.iter().position(|row| matches!(row, Row::Song(i) if &app.songs[*i].id == id))) {
                let height = ui.available_height();
                let top = row as f32 * (ROW + ui.spacing().item_spacing.y);
                let offset: f32 = ui.ctx().data(|d| d.get_temp(egui::Id::new("songs-offset"))).unwrap_or(0.0);
                if top < offset || top + ROW > offset + height {
                    scroll = scroll.vertical_scroll_offset((top - height / 2.0 + ROW).max(0.0));
                }
            }
        }
        let mut play = None;
        let mut heart = None;
        let mut group_click = None;
        let mut action: Option<(&str, String)> = None;
        let moving = app.player.is_playing();
        let output = scroll.show_rows(ui, ROW, app.rows.len(), |ui, range| {
            for row in &app.rows[range] {
                match row {
                    Row::Group { key, name, sub, count } => {
                        let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                        let painter = ui.painter();
                        let clickable = matches!(app.prefs.sort.as_str(), "artist" | "genre");
                        let band = Rect::from_min_max(pos2(rect.min.x, rect.max.y - 34.0), pos2(rect.max.x, rect.max.y - 2.0));
                        painter.rect_filled(band, 10.0, if clickable && response.hovered() { t.accent_soft.gamma_multiply(1.6) } else { t.accent_soft });
                        let color = t.accent;
                        let bottom = rect.max.y - 8.0;
                        let galley = painter.layout_no_wrap(name.clone(), bold(15.0), color);
                        let name_width = galley.size().x.min(rect.width() - 80.0);
                        clipped_text(painter, Rect::from_min_max(pos2(rect.min.x + 8.0, bottom - 20.0), pos2(rect.min.x + 8.0 + name_width, bottom)), name, bold(15.0), color);
                        if !sub.is_empty() {
                            clipped_text(painter, Rect::from_min_max(pos2(rect.min.x + 16.0 + name_width, bottom - 20.0), pos2(rect.max.x - 60.0, bottom)), sub, regular(12.0), t.muted);
                        }
                        if app.prefs.sort != "title" {
                            painter.text(pos2(rect.max.x - 8.0, bottom - 10.0), Align2::RIGHT_CENTER, plural(*count, "song"), regular(12.0), t.muted);
                        }
                        if clickable {
                            let tip = if app.prefs.sort == "artist" {
                                if app.artist_filter.is_some() { "Show all artists".to_string() } else { format!("Show only {name}") }
                            } else if app.genre_filter.is_some() { "Show all genres".into() } else { format!("Show only {name}") };
                            if response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                group_click = Some(key.clone());
                            }
                        }
                    }
                    Row::Song(i) => {
                        let song = &app.songs[*i];
                        let playing = app.current.as_deref() == Some(song.id.as_str());
                        let response = song_line(ui, t, &song.title, &song.artist, playing, false, moving);
                        // On the right: "!" for a song that's here twice, and (when pointed at)
                        // the pencil and the trash, on a backing so the names don't show through
                        let duplicate = app.duplicate_group(&song.id).is_some();
                        let row_hovered = ui.rect_contains_pointer(response.rect);
                        let mut x = response.rect.max.x - 48.0;
                        let mut small = |ui: &mut Ui, glyph: &str, color: Color32, tip: &str| {
                            let rect = Rect::from_center_size(pos2(x, response.rect.center().y), Vec2::splat(28.0));
                            x -= 28.0;
                            let button = ui.interact(rect, ui.id().with((glyph, &song.id)), Sense::click());
                            ui.painter().circle_filled(rect.center(), 14.0, if button.hovered() { t.track } else { t.surface_strong });
                            ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, icons(16.0), color);
                            button.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked()
                        };
                        if duplicate && small(ui, icon::WARNING, t.warning, "This song is here more than once. Click to choose which copies to keep") {
                            action = Some(("duplicates", song.id.clone()));
                        }
                        if row_hovered && !app.busy {
                            if small(ui, icon::PENCIL_SIMPLE, t.muted, "Edit title, artist, album and genre") {
                                action = Some(("edit", song.id.clone()));
                            }
                            if small(ui, icon::TRASH, t.danger, "Delete this song") {
                                action = Some(("delete", song.id.clone()));
                            }
                        }
                        // The heart: always shown on favorites, on the others when pointed at
                        let favorite = app.is_favorite(&song.id);
                        let heart_rect = Rect::from_center_size(pos2(response.rect.max.x - 20.0, response.rect.center().y), Vec2::splat(28.0));
                        let heart_response = ui.interact(heart_rect, ui.id().with(("heart", &song.id)), Sense::click());
                        if favorite || response.hovered() || heart_response.hovered() {
                            let (glyph, font, color) = if favorite { (egui_phosphor::fill::HEART, filled(17.0), t.accent) } else { (icon::HEART, icons(17.0), t.muted) };
                            if heart_response.hovered() {
                                ui.painter().circle_filled(heart_rect.center(), 14.0, t.hover);
                            }
                            ui.painter().text(heart_rect.center(), Align2::CENTER_CENTER, glyph, font, color);
                        }
                        if heart_response.on_hover_text(if favorite { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                            heart = Some(song.id.clone());
                        } else if response.clicked() && action.is_none() {
                            play = Some(song.id.clone());
                        }
                    }
                }
            }
        });
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("songs-offset"), output.state.offset.y));
        if let Some(id) = heart {
            app.toggle_favorite(&id);
        }
        match action {
            Some(("edit", id)) => app.start_editing(&id),
            Some(("delete", id)) => app.deleting = Some(id),
            Some(("duplicates", id)) => app.open_duplicates(&id),
            _ => {}
        }
        if let Some(id) = play {
            let list = app.shown_ids();
            app.play_from_list(&id, list);
        }
        if let Some(key) = group_click {
            if app.prefs.sort == "artist" { app.filter_by_artist(&key) } else { app.filter_by_genre(&key) }
        }
    });
}

fn sort_menu(app: &mut App, ctx: &egui::Context, button: Rect, t: &Theme) {
    let area = egui::Area::new(egui::Id::new("sort-menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(button.max.x, button.max.y + 6.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.set_width(210.0);
                heading(ui, t, "Sort");
                for (id, label) in SORTS {
                    let chosen = app.prefs.sort == id;
                    if menu_item(ui, t, label, chosen).clicked() {
                        app.set_sort(id);
                        app.sort_open = false;
                    }
                }
                ui.add_space(4.0);
                ui.separator();
                heading(ui, t, "Show");
                let only = app.prefs.only_favorites;
                if menu_item(ui, t, "Only favorites", only).clicked() {
                    app.set_only_favorites(!only);
                    app.sort_open = false;
                }
            });
        });
    if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| button.contains(p))) {
        app.sort_open = false;
    }
}

fn menu_item(ui: &mut Ui, t: &Theme, label: &str, chosen: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 32.0), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 8.0, t.hover);
    }
    ui.painter().text(pos2(rect.min.x + 10.0, rect.center().y), Align2::LEFT_CENTER, label, if chosen { semibold(14.0) } else { regular(14.0) }, t.text);
    if chosen {
        ui.painter().text(pos2(rect.max.x - 12.0, rect.center().y), Align2::CENTER_CENTER, icon::CHECK, icons(15.0), t.accent);
    }
    response
}

// ---------- The song that's playing ----------

fn now_playing(app: &mut App, ui: &mut Ui, t: &Theme) {
    let Some(song) = app.current_song() else {
        // Nothing yet: what the folder holds
        ui.centered_and_justified(|ui| {
            let message = if !app.listed {
                "Reading your music folder…".to_string()
            } else if app.tags_waiting > 0 {
                format!("Reading song info… {} to go", app.tags_waiting)
            } else if app.songs.is_empty() {
                "No songs here yet. Copy some into your music folder, or pick another one with \"Set default folder\".".into()
            } else {
                format!("{} ready. Search above or pick one from the list.", plural(app.songs.len(), "song"))
            };
            ui.label(RichText::new(message).font(regular(16.0)).color(t.muted));
        });
        return;
    };
    let (id, title, artist, album, genre) = (song.id.clone(), song.title.clone(), song.artist.clone(), song.album.clone(), song.genre.clone());
    let known_artist = song.known_artist();
    app.request_cover(&id, true);
    let texture = app.textures.get(&id).and_then(|c| c.big.as_ref().or(c.small.as_ref())).cloned();

    let available = ui.available_size();
    let size = (available.x - 60.0).min(available.y - 190.0).clamp(160.0, 460.0);
    ui.vertical_centered(|ui| {
        ui.add_space(((available.y - size - 170.0) / 2.0).max(8.0));
        // The cover, with a soft shadow
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
        let shadow = egui::Shadow { offset: [0, 18], blur: 48, spread: 0, color: Color32::from_black_alpha(if t.dark { 120 } else { 55 }) };
        ui.painter().add(shadow.as_shape(rect, CornerRadius::same(22)));
        paint_cover(ui.painter(), t, texture.as_ref(), rect, 22);
        ui.add_space(22.0);

        ui.add(egui::Label::new(RichText::new(&title).font(bold(30.0))).wrap().halign(Align::Center));
        ui.add_space(2.0);
        let artist_label = egui::Label::new(RichText::new(&artist).font(regular(18.0)).color(t.muted)).sense(Sense::click());
        let response = ui.add(artist_label);
        if known_artist && response.on_hover_text("Show only this artist").on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
            let key = names::normalize(&artist);
            if app.artist_filter.as_deref() != Some(key.as_str()) {
                app.filter_by_artist(&key);
            }
        }
        if !album.is_empty() {
            ui.label(RichText::new(&album).font(regular(13.5)).color(t.muted));
        }
        ui.add_space(10.0);
        equalizer(app, ui, t, size);
        let genres = names::genres_of(&genre);
        if !genres.is_empty() {
            ui.add_space(10.0);
            // The song's genres in a row (centered)
            let widths: f32 = genres.iter().map(|g| ui.fonts_mut(|f| f.layout_no_wrap(g.to_string(), semibold(12.0), t.text).size().x) + 30.0).sum();
            ui.horizontal(|ui| {
                ui.add_space(((ui.available_width() - widths) / 2.0).max(0.0));
                for genre in genres {
                    Frame::NONE.fill(t.accent_soft).corner_radius(CornerRadius::same(12)).inner_margin(Margin::symmetric(10, 4)).show(ui, |ui| {
                        ui.label(RichText::new(genre).font(semibold(12.0)).color(t.accent));
                    });
                }
            });
        }
    });
}

// ---------- The player bar ----------

fn player_bar(app: &mut App, ui: &mut Ui, t: &Theme) {
    Frame::NONE
        .fill(t.surface_strong)
        .corner_radius(CornerRadius::same(20))
        .inner_margin(Margin::symmetric(16, 10))
        .show(ui, |ui| {
            ui.set_height(64.0);
            let width = ui.available_width();
            ui.columns_const(|[left, middle, right]| {
                // The song: its cover, names and heart
                left.set_width(width * 0.27);
                left.horizontal(|ui| {
                    if let Some(song) = app.current_song() {
                        let (id, title, artist) = (song.id.clone(), song.title.clone(), song.artist.clone());
                        let texture = app.textures.get(&id).and_then(|c| c.small.clone());
                        cover(ui, t, texture.as_ref(), 54.0, 12);
                        ui.vertical(|ui| {
                            ui.add_space(8.0);
                            ui.set_width((ui.available_width() - 44.0).max(60.0));
                            ui.add(egui::Label::new(RichText::new(&title).font(semibold(14.5))).truncate());
                            ui.add(egui::Label::new(RichText::new(&artist).font(regular(12.5)).color(t.muted)).truncate());
                        });
                        let favorite = app.is_favorite(&id);
                        let (glyph, color) = if favorite { (egui_phosphor::fill::HEART, t.accent) } else { (icon::HEART, t.muted) };
                        let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
                        if response.hovered() {
                            ui.painter().circle_filled(rect.center(), 18.0, t.hover);
                        }
                        ui.painter().text(rect.center(), Align2::CENTER_CENTER, glyph, if favorite { filled(19.0) } else { icons(19.0) }, color);
                        if response.on_hover_text(if favorite { "Remove from favorites" } else { "Add to favorites" }).clicked() {
                            app.toggle_favorite(&id);
                        }
                    }
                });

                // The buttons and where the song is
                middle.vertical_centered(|ui| {
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 228.0).max(0.0) / 2.0);
                        let shuffle_color = if app.prefs.shuffle { t.accent } else { t.muted };
                        let response = icon_button(ui, t, icon::SHUFFLE, 38.0, shuffle_color, if app.prefs.shuffle { "Shuffle is on" } else { "Shuffle" });
                        if app.prefs.shuffle {
                            ui.painter().circle_filled(pos2(response.rect.center().x, response.rect.max.y - 3.0), 2.0, t.accent);
                        }
                        if response.clicked() {
                            app.toggle_shuffle();
                        }
                        if icon_button(ui, t, icon::SKIP_BACK, 38.0, t.text, "Previous").clicked() {
                            app.play_previous();
                        }
                        // Play / pause: a round button in the accent colors
                        let (rect, response) = ui.allocate_exact_size(Vec2::splat(46.0), Sense::click());
                        let grow = if response.hovered() { 1.0 } else { 0.0 };
                        let painter = ui.painter();
                        painter.circle_filled(rect.center(), 23.0 + grow, t.accent2);
                        painter.circle_filled(rect.center() - vec2(1.5, 1.5), 21.5 + grow, t.accent);
                        let glyph = if app.player.is_playing() { egui_phosphor::fill::PAUSE } else { egui_phosphor::fill::PLAY };
                        painter.text(rect.center(), Align2::CENTER_CENTER, glyph, filled(20.0), t.on_accent);
                        if response.on_hover_text(if app.player.is_playing() { "Pause (Space)" } else { "Play (Space)" }).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                            app.toggle_play();
                        }
                        if icon_button(ui, t, icon::SKIP_FORWARD, 38.0, t.text, "Next").clicked() {
                            app.play_next();
                        }
                        ui.add_space(38.0); // as wide as shuffle, so play stays in the middle
                    });
                    ui.horizontal(|ui| {
                        let position = if app.current.is_some() { app.player.position() } else { 0.0 };
                        let duration = app.duration.max(position);
                        ui.label(RichText::new(format_time(position)).font(regular(12.0)).color(t.muted));
                        let width = (ui.available_width() - 44.0).max(80.0);
                        if let Some(to) = slider(ui, t, position, duration, width) {
                            app.player.seek(to);
                        }
                        ui.label(RichText::new(format_time(duration)).font(regular(12.0)).color(t.muted));
                    });
                });

                // Volume
                right.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(4.0);
                    let volume = if app.prefs.muted { 0.0 } else { app.prefs.volume };
                    if let Some(to) = slider(ui, t, volume, 1.0, 120.0) {
                        app.set_volume(to);
                    }
                    let glyph = if volume == 0.0 { icon::SPEAKER_X } else if volume < 0.5 { icon::SPEAKER_LOW } else { icon::SPEAKER_HIGH };
                    if icon_button(ui, t, glyph, 36.0, t.muted, if app.prefs.muted { "Sound on" } else { "Mute" }).clicked() {
                        app.toggle_mute();
                    }
                    // Even volume: lit (with a dot under it) while it's on
                    let on = app.prefs.even_volume;
                    let (rect, response) = ui.allocate_exact_size(Vec2::splat(36.0), Sense::click());
                    if response.hovered() {
                        ui.painter().circle_filled(rect.center(), 18.0, t.hover);
                    }
                    let color = if on { t.accent } else if response.hovered() { t.text } else { t.muted };
                    even_icon(ui.painter(), rect.center(), color);
                    if on {
                        ui.painter().circle_filled(pos2(rect.center().x, rect.max.y - 3.0), 2.0, t.accent);
                    }
                    let tip = if on { "Even volume is on: songs play at about the same loudness" } else { "Even volume" };
                    if response.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                        app.toggle_even_volume();
                    }
                });
            });
        });
}

// The Even volume icon: bars of the same height between two rails
fn even_icon(painter: &egui::Painter, center: Pos2, color: Color32) {
    for y in [-7.0, 7.0] {
        painter.rect_filled(Rect::from_center_size(center + vec2(0.0, y), vec2(16.0, 1.6)), 0.8, color);
    }
    for x in [-5.0, -1.7, 1.7, 5.0] {
        painter.rect_filled(Rect::from_center_size(center + vec2(x, 0.0), vec2(1.8, 6.0)), 0.9, color);
    }
}

// The equalizer: bars rising from the middle line, in the theme's two accent colors;
// small dots while it's quiet
fn equalizer(app: &App, ui: &mut Ui, t: &Theme, width: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(width, 40.0), Sense::hover());
    let painter = ui.painter();
    let step = rect.width() / BARS as f32;
    for (i, level) in app.spectrum.levels.iter().enumerate() {
        let x = rect.min.x + step * (i as f32 + 0.5);
        let mix = i as f32 / (BARS - 1) as f32;
        let color = lerp_color(t.accent, t.accent2, mix);
        let height = level * rect.height();
        if height < 3.0 {
            painter.circle_filled(pos2(x, rect.center().y), 1.6, color.gamma_multiply(0.7));
        } else {
            painter.rect_filled(Rect::from_center_size(pos2(x, rect.center().y), vec2((step * 0.5).min(5.0), height)), 2.0, color);
        }
    }
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_premultiplied(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()), mix(a.a(), b.a()))
}

// ---------- Lyrics ----------

fn lyrics_card(app: &mut App, ui: &mut Ui, t: &Theme) {
    card(ui, t, |ui| {
        let has_song = app.current.is_some();
        let found = app.lyrics.as_ref().and_then(|view| view.found.clone());
        let offset = app.lyrics.as_ref().map_or(0.0, |view| view.offset);
        let timed = app.lyrics.as_ref().is_some_and(|view| !view.lines.is_empty());
        ui.horizontal(|ui| {
            ui.set_height(30.0);
            heading(ui, t, "Lyrics");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if !has_song {
                    return;
                }
                // Save: lyrics found online, or moved ones
                let can_save = found.as_ref().is_some_and(|f| (f.synced.is_some() || f.plain.is_some()) && (!f.saved || offset != 0.0));
                if can_save {
                    let tip = if offset != 0.0 { "Save these lyrics with the new timing" } else { "Save these lyrics in the lyrics folder" };
                    if icon_button(ui, t, icon::DOWNLOAD_SIMPLE, 30.0, t.muted, tip).clicked() {
                        app.save_lyrics();
                    }
                }
                // Where they come from (a menu to pick another source)
                let name = match &found {
                    Some(f) if !f.source_name.is_empty() => f.source_name.clone(),
                    Some(_) => "Sources".into(),
                    None => "Looking…".into(),
                };
                let button = egui::Button::new(with_icon(icon::CARET_DOWN, &name, semibold(12.0), t.accent))
                    .fill(t.accent_soft)
                    .corner_radius(CornerRadius::same(13))
                    .min_size(vec2(0.0, 26.0));
                let response = ui.add(button).on_hover_text("Where the lyrics come from");
                ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("lyrics-button"), response.rect));
                if response.clicked() {
                    app.lyrics_menu_open = !app.lyrics_menu_open;
                }
            });
        });
        ui.add_space(4.0);

        let message = |ui: &mut Ui, text: &str| {
            ui.add_space(30.0);
            ui.vertical_centered(|ui| ui.label(RichText::new(text).font(regular(15.0)).color(t.muted)));
        };
        if !has_song {
            message(ui, "Lyrics show up here when a song is playing.");
            return;
        }
        let Some(found) = found else {
            message(ui, "Looking for lyrics…");
            return;
        };

        // The timing bar sits at the bottom, under the lyrics
        let tapping = app.lyrics.as_ref().is_some_and(|v| v.tapping.is_some());
        let bottom_space = if tapping { 96.0 } else if timed { 44.0 } else { 0.0 };
        let lyrics_height = (ui.available_height() - bottom_space).max(60.0);
        if timed {
            if found.other_version {
                ui.label(RichText::new("Timed lyrics of a slightly different version: Timing below can line them up.").font(regular(11.5)).color(t.muted));
            }
            timed_lyrics(app, ui, t, lyrics_height - if found.other_version { 30.0 } else { 0.0 });
            if app.lyrics.as_ref().is_some_and(|v| v.tapping.is_some()) { tap_bar(app, ui, t) } else { timing_bar(app, ui, t) }
        } else if let Some(text) = found.plain.as_ref().or(found.synced.as_ref()) {
            if found.other_version {
                ui.label(RichText::new("These are the lyrics of another version of the song.").font(regular(11.5)).color(t.muted));
            }
            ui.horizontal(|ui| {
                ui.label(RichText::new("No timing for these.").font(regular(12.0)).color(t.muted));
                if ui.add(egui::Button::new(with_icon(icon::TIMER, "Make timing", semibold(12.5), t.accent)).fill(t.accent_soft).corner_radius(CornerRadius::same(12)))
                    .on_hover_text("A first guess, then tap along as the lines start to fix it").clicked() {
                    app.start_tapping();
                }
            });
            let id = app.lyrics.as_ref().map_or(String::new(), |view| view.id.clone());
            egui::ScrollArea::vertical().id_salt(("plain-lyrics", id)).auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(text).font(regular(15.0)).line_height(Some(24.0))).wrap());
            });
        } else {
            message(ui, found.message.as_deref().unwrap_or("No lyrics found for this song."));
        }
    });
}

// Timed lyrics: the line being sung is highlighted and kept in the middle; click a line
// to jump there. Scrolling yourself pauses the following for a few seconds.
fn timed_lyrics(app: &mut App, ui: &mut Ui, t: &Theme, height: f32) {
    let position = app.player.position();
    let Some(view) = app.lyrics.as_mut() else { return };
    let offset = view.offset;
    let active = view.lines.iter().rposition(|line| line.time + offset <= position);
    let next = view.tapping.as_ref().map(|tapping| tapping.next);
    let mut set_now = None;
    let last_active: Option<usize> = ui.ctx().data(|d| d.get_temp(egui::Id::new("lyrics-active"))).flatten();
    let follow = active != last_active && view.scrolled_at.is_none_or(|at| at.elapsed() > Duration::from_secs(4));
    let mut jump = None;
    // Each song's lyrics scroll on their own (a new song starts at the top), and before
    // the first line is sung they stay at the top
    let mut scroll = egui::ScrollArea::vertical().id_salt(("timed-lyrics", &view.id)).max_height(height).auto_shrink([false, false]);
    if active.is_none() && follow {
        scroll = scroll.vertical_scroll_offset(0.0);
    }
    let output = scroll.show(ui, |ui| {
        ui.add_space(8.0);
        for (index, line) in view.lines.iter().enumerate() {
            let is_active = Some(index) == active;
            let text = if line.text.is_empty() { "♪" } else { &line.text };
            // The line being sung glides in: it brightens and grows a little, and the one
            // before it fades back (instead of switching in one frame)
            let glow = ui.ctx().animate_bool_with_time_and_easing(
                ui.id().with(("lyric-line", &view.id, index)), is_active, 0.45, egui::emath::easing::cubic_in_out);
            let size = 16.0 + 2.0 * glow;
            let rich = if Some(index) == next {
                RichText::new(format!("▶ {text}")).font(bold(18.0)).color(t.accent) // tap when this one starts
            } else {
                let font = if glow > 0.5 { bold(size) } else { semibold(size) };
                RichText::new(text).font(font).color(lerp_color(t.muted, t.text, glow))
            };
            let response = ui.add(egui::Label::new(rich).wrap().sense(Sense::click())).on_hover_cursor(egui::CursorIcon::PointingHand);
            ui.add_space(6.0);
            if is_active && follow {
                // an eased glide to the middle, a little slower than egui's usual jump
                response.scroll_to_me_animation(Some(Align::Center), egui::style::ScrollAnimation::duration(0.6));
            }
            if response.clicked() {
                if next.is_some() { set_now = Some(index) } else { jump = Some((line.time + offset).max(0.0)) }
            }
        }
        ui.add_space(8.0);
    });
    if follow {
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("lyrics-active"), active));
    }
    if ui.rect_contains_pointer(output.inner_rect) && ui.input(|i| i.smooth_scroll_delta.y != 0.0) {
        view.scrolled_at = Some(std::time::Instant::now());
    }
    if let Some(index) = set_now {
        app.tap(Some(index)); // while tapping, a clicked line starts now
    }
    if let Some(seconds) = jump {
        app.player.seek(seconds);
        if let Some(view) = app.lyrics.as_mut() {
            view.scrolled_at = None;
        }
    }
}

// Tapping along: Space or Tap as the highlighted line starts
fn tap_bar(app: &mut App, ui: &mut Ui, t: &Theme) {
    ui.separator();
    ui.label(RichText::new("Press Space (or Tap) as the highlighted line starts. Click any line to set it to now.").font(regular(11.5)).color(t.muted));
    ui.horizontal(|ui| {
        if primary_button(ui, t, "Tap", true) {
            app.tap(None);
        }
        if plain_button(ui, t, "Undo") {
            app.undo_tap();
        }
        if plain_button(ui, t, "Done") {
            app.finish_tapping();
        }
    });
}

// Timing, like a video player's subtitle delay: - shows the lyrics earlier, + later
fn timing_bar(app: &mut App, ui: &mut Ui, t: &Theme) {
    let offset = app.lyrics.as_ref().map_or(0.0, |view| view.offset);
    ui.separator();
    ui.horizontal(|ui| {
        ui.add_space(((ui.available_width() - 200.0) / 2.0).max(0.0));
        ui.label(RichText::new("Timing").font(regular(12.0)).color(t.muted));
        if icon_button(ui, t, icon::MINUS, 30.0, t.muted, "Show the lyrics 0.5 s earlier (G: 0.1 s)").clicked() {
            app.move_lyrics(-0.5);
        }
        let sign = if offset > 0.0 { "+" } else if offset < 0.0 { "\u{2212}" } else { "" };
        let label = RichText::new(format!("{sign}{:.1} s", offset.abs())).font(if offset != 0.0 { semibold(13.0) } else { regular(13.0) }).color(if offset != 0.0 { t.accent } else { t.muted });
        let tip = if offset == 0.0 { "How much the lyrics are moved".to_string() } else { format!("The lyrics are {:.1} s {}. Click to undo", offset.abs(), if offset > 0.0 { "later" } else { "earlier" }) };
        if ui.add(egui::Button::new(label).frame(false).min_size(vec2(64.0, 26.0))).on_hover_text(tip).clicked() {
            app.reset_lyrics_timing();
        }
        if icon_button(ui, t, icon::PLUS, 30.0, t.muted, "Show the lyrics 0.5 s later (H: 0.1 s)").clicked() {
            app.move_lyrics(0.5);
        }
        if icon_button(ui, t, icon::TIMER, 30.0, t.muted, "Fix the timing by tapping along").clicked() {
            app.start_tapping();
        }
    });
}

// Where the lyrics come from: "Best match", or one source (kept for this song)
fn lyrics_menu(app: &mut App, ctx: &egui::Context, t: &Theme) {
    if !app.lyrics_menu_open {
        return;
    }
    let button: Rect = ctx.data(|d| d.get_temp(egui::Id::new("lyrics-button"))).unwrap_or(Rect::NOTHING);
    let choice = app.current.as_ref().and_then(|id| app.library.get(id)).and_then(|record| record.lyrics.clone());
    let mut picked: Option<Option<&str>> = None;
    let area = egui::Area::new(egui::Id::new("lyrics-menu"))
        .order(egui::Order::Foreground)
        .pivot(Align2::RIGHT_TOP)
        .fixed_pos(pos2(button.max.x, button.max.y + 6.0))
        .show(ctx, |ui| {
            Frame::popup(ui.style()).inner_margin(Margin::same(8)).show(ui, |ui| {
                ui.set_width(200.0);
                heading(ui, t, "Lyrics from");
                if menu_item(ui, t, "Best match", choice.is_none()).clicked() {
                    picked = Some(None);
                }
                for (id, name) in crate::lyrics::SOURCES {
                    if menu_item(ui, t, name, choice.as_deref() == Some(id)).clicked() {
                        picked = Some(Some(id));
                    }
                }
            });
        });
    if let Some(source) = picked {
        app.lyrics_menu_open = false;
        app.pick_lyrics_source(source);
    } else if area.response.clicked_elsewhere() && !ctx.input(|i| i.pointer.interact_pos().is_some_and(|p| button.contains(p))) {
        app.lyrics_menu_open = false;
    }
}

// ---------- Changing songs ----------

fn window_frame(ctx: &egui::Context, t: &Theme) -> Frame {
    Frame::popup(&ctx.style()).fill(t.menu).corner_radius(CornerRadius::same(20)).inner_margin(Margin::same(24))
}

fn primary_button(ui: &mut Ui, t: &Theme, text: &str, enabled: bool) -> bool {
    let button = egui::Button::new(RichText::new(text).font(semibold(14.0)).color(t.on_accent)).fill(t.accent).corner_radius(CornerRadius::same(18)).min_size(vec2(0.0, 36.0));
    ui.add_enabled(enabled, button).clicked()
}

fn plain_button(ui: &mut Ui, t: &Theme, text: &str) -> bool {
    let button = egui::Button::new(RichText::new(text).font(semibold(14.0))).fill(t.hover).corner_radius(CornerRadius::same(18)).min_size(vec2(0.0, 36.0));
    ui.add(button).clicked()
}

// A text field with suggestions from the library under it while you type. For a list
// ("Rock, Indie") the suggestions are for its last name.
fn field(ui: &mut Ui, t: &Theme, label: &str, text: &mut String, options: &[String], list: bool) {
    ui.label(RichText::new(label).font(semibold(12.0)).color(t.muted));
    let response = ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY).font(regular(15.0)).margin(vec2(10.0, 8.0)));
    if options.is_empty() {
        ui.add_space(6.0);
        return;
    }
    // Suggestions go in a row of their own that's always there (empty when there are
    // none), so the window never jumps while you type
    let typed = if list { text.rsplit(',').next().unwrap_or("").trim().to_string() } else { text.trim().to_string() };
    let typed_key = names::normalize(&typed);
    let picks: Vec<&String> = if response.has_focus() && !typed_key.is_empty() {
        options.iter().filter(|o| names::normalize(o).contains(&typed_key) && names::normalize(o) != typed_key).take(4).collect()
    } else {
        Vec::new()
    };
    let (row, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let mut chips = ui.new_child(egui::UiBuilder::new().max_rect(row).layout(Layout::left_to_right(Align::Center)));
    for pick in picks {
        let chip = egui::Button::new(RichText::new(pick.as_str()).font(regular(12.5))).fill(t.hover).corner_radius(CornerRadius::same(12));
        if chips.add(chip).clicked() {
            if list {
                let mut parts: Vec<String> = text.split(',').map(|p| p.trim().to_string()).collect();
                parts.pop();
                parts.push(pick.clone());
                *text = parts.join(", ");
            } else {
                *text = pick.clone();
            }
            response.request_focus();
        }
    }
}

// The pencil: a song's names, saved in library.json, its tags and (if ticked) its file name
fn edit_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some(mut form) = app.editing.take() else { return };
    let Some(song) = app.song(&form.id) else { return };
    let natural = song.natural.clone();
    let old_name = song.rel.rsplit('/').next().unwrap_or("").to_string();
    // Suggestions: names already in the library, most songs first
    let artists: Vec<String> = app.stats.artists.iter().filter(|b| !b.key.is_empty()).map(|b| b.name.clone()).collect();
    let genres: Vec<String> = app.stats.genres.iter().filter(|b| !b.key.is_empty()).map(|b| b.name.clone()).collect();
    let mut albums: Vec<String> = app.songs.iter().filter(|s| !s.album.is_empty()).map(|s| s.album.clone()).collect();
    albums.sort();
    albums.dedup();
    let offer = app.rename_offer(&form);
    let (mut save, mut cancel, mut original, mut from_wiki) = (false, false, false, false);
    egui::Modal::new(egui::Id::new("edit")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new("Edit song").font(bold(20.0)));
        ui.label(RichText::new(&old_name).font(regular(12.0)).color(t.muted));
        ui.add_space(12.0);
        field(ui, t, "Title", &mut form.title, &[], false);
        field(ui, t, "Artist", &mut form.artist, &artists, false);
        ui.horizontal(|ui| {
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let looking = app.wiki_fill.is_some();
                let label = if looking { "Looking it up…" } else { "Album and genres from Wikipedia" };
                if ui.add_enabled(!looking, egui::Button::new(with_icon(icon::DISC, label, regular(12.5), t.accent)).frame(false)).clicked() {
                    from_wiki = true;
                }
            });
        });
        field(ui, t, "Album", &mut form.album, &albums, false);
        field(ui, t, "Genres (separate several with commas)", &mut form.genre, &genres, true);
        if !offer.is_empty() {
            ui.checkbox(&mut form.rename, RichText::new(format!("Also rename the file to \"{offer}\"")).font(regular(13.0)));
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            save = primary_button(ui, t, "Save", !form.title.trim().is_empty());
            cancel = plain_button(ui, t, "Cancel");
            let differs = form.title.trim() != natural.title || form.artist.trim() != if natural.artist == UNKNOWN_ARTIST { "" } else { natural.artist.as_str() }
                || form.album.trim() != natural.album || form.genre.trim() != natural.genre;
            if differs {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(RichText::new("Use the original names").font(regular(13.0)).color(t.muted)).frame(false))
                        .on_hover_text(format!("{} · {}", natural.title, natural.artist)).clicked() {
                        original = true;
                    }
                });
            }
        });
        if ui.input(|i| i.key_pressed(egui::Key::Enter)) && !form.title.trim().is_empty() {
            save = true;
        }
    });
    if from_wiki {
        app.editing = Some(form);
        app.wiki_for_form();
    } else if original {
        let id = form.id.clone();
        app.use_original_names(&id);
    } else if save {
        app.editing = Some(form);
        app.save_edit();
    } else if !cancel && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.editing = Some(form);
    }
}

// "Tidy file names": every file named "Artist - Title", and names you gave songs
// written into their files. Only what's ticked changes, and the last rename can be undone.
fn tidy_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    if app.tidy.is_none() {
        return;
    }
    let no_artist = app.songs.iter().filter(|s| !s.known_artist()).count();
    let can_undo = app.can_undo();
    let (mut apply, mut close, mut undo) = (false, false, false);
    egui::Modal::new(egui::Id::new("tidy")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(620.0);
        let list = app.tidy.as_mut().unwrap();
        let possible = list.iter().filter(|item| !item.clash || item.tags).count();
        ui.label(RichText::new("Tidy file names").font(bold(20.0)));
        ui.label(RichText::new(if possible == 0 {
            "Every file is named \"Artist - Title\" already, and your names are in the files.".to_string()
        } else {
            format!("{} can be named \"Artist - Title\" or get the names you gave them. Untick any you'd rather keep as they are.", plural(possible, "file"))
        }).color(t.muted));
        if no_artist > 0 {
            ui.label(RichText::new(format!("{} no artist yet: give them one with the pencil, and they can be named \"Artist - Title\" too.", plural(no_artist, "song has").replace("songs has", "songs have"))).font(regular(12.5)).color(t.muted));
        }
        ui.add_space(8.0);
        if possible > 0 {
            ui.horizontal(|ui| {
                let all = list.iter().filter(|i| !i.clash || i.tags).all(|i| i.chosen);
                if ui.add(egui::Button::new(RichText::new(if all { "Untick all" } else { "Tick all" }).font(regular(12.5))).frame(false)).clicked() {
                    for item in list.iter_mut().filter(|i| !i.clash || i.tags) {
                        item.chosen = !all;
                    }
                }
            });
            let height = (list.len() as f32 * 52.0).min(380.0);
            egui::ScrollArea::vertical().max_height(380.0).min_scrolled_height(height).show(ui, |ui| {
                for item in list.iter_mut() {
                    ui.horizontal(|ui| {
                        ui.add_enabled_ui(!item.clash || item.tags, |ui| ui.checkbox(&mut item.chosen, ""));
                        ui.vertical(|ui| {
                            let renamed = item.new_name != item.old_name;
                            if renamed {
                                ui.label(RichText::new(&item.old_name).font(regular(12.5)).color(t.muted).strikethrough());
                            }
                            let note = if item.clash && item.tags { "  (only the names inside: that file name is taken)" }
                                else if item.clash { "  (skipped: that name is taken)" }
                                else if item.tags && renamed { "  + the names inside the file" }
                                else if item.tags { "  (the names inside the file)" } else { "" };
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new(&item.new_name).font(semibold(13.5)));
                                if !note.is_empty() {
                                    ui.label(RichText::new(note).font(regular(12.0)).color(t.muted));
                                }
                            });
                        });
                    });
                    ui.add_space(4.0);
                }
            });
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let count = list.iter().filter(|i| i.chosen && (!i.clash || i.tags)).count();
            let verb = if list.iter().any(|i| i.chosen && i.tags) { "Save" } else { "Rename" };
            if !list.is_empty() {
                apply = primary_button(ui, t, &format!("{verb} {}", plural(count, "file")), count > 0);
            }
            close = plain_button(ui, t, if list.is_empty() { "Close" } else { "Cancel" });
            if can_undo {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui.add(egui::Button::new(with_icon(icon::ARROW_COUNTER_CLOCKWISE, "Undo last rename", regular(13.0), t.muted)).frame(false)).clicked() {
                        undo = true;
                    }
                });
            }
        });
    });
    if apply {
        app.apply_tidy();
    } else if undo {
        app.undo_rename();
    } else if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.tidy = None;
    }
}

fn delete_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some(id) = app.deleting.clone() else { return };
    let Some(song) = app.song(&id) else {
        app.deleting = None;
        return;
    };
    let (title, artist) = (song.title.clone(), song.artist.clone());
    let (mut delete, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("delete")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new("Delete this song?").font(bold(20.0)));
        ui.add_space(6.0);
        ui.label(RichText::new(format!("\"{title}\" by {artist} goes to the Trash, so you can still get it back from there.")).color(t.muted));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let button = egui::Button::new(RichText::new("Move to Trash").font(semibold(14.0)).color(Color32::WHITE)).fill(t.danger).corner_radius(CornerRadius::same(18)).min_size(vec2(0.0, 36.0));
            delete = ui.add(button).clicked();
            cancel = plain_button(ui, t, "Cancel");
        });
    });
    if delete {
        app.delete_song(&id);
    } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.deleting = None;
    }
}

// Copies of one song: the best first (lossless, then the highest bitrate, then the
// biggest file). Ticked copies go to the Trash; their hearts and plays go to the one kept.
fn duplicates_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some(mut dialog) = app.duplicates.take() else { return };
    let rows: Vec<(String, String, String)> = dialog.ids.iter().map(|id| match app.song(id) {
        Some(song) => {
            let extension = song.rel.rsplit('.').next().unwrap_or("").to_uppercase();
            let details = format!("{extension} · {} kbps · {} · {:.1} MB · plays: {}", song.bitrate, format_time(song.duration), song.size as f64 / 1e6, app.plays_of(id));
            (song.rel.clone(), details, song.title.clone())
        }
        None => (id.clone(), String::new(), String::new()),
    }).collect();
    let longest = dialog.ids.first().and_then(|id| app.song(id)).map_or(0.0, |s| s.duration);
    let (mut remove, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("duplicates")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(560.0);
        ui.label(RichText::new(format!("\"{}\" is here {} times", rows.first().map_or("", |r| r.2.as_str()), rows.len())).font(bold(20.0)));
        ui.label(RichText::new("The best copy is first. Ticked copies go to the Trash; their hearts, plays and lyrics choice go to the copy you keep.").color(t.muted));
        ui.add_space(10.0);
        for (index, (rel, details, _)) in rows.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.checkbox(&mut dialog.remove[index], "");
                ui.vertical(|ui| {
                    ui.label(RichText::new(rel).font(semibold(13.5)));
                    let mut note = details.clone();
                    if index == 0 {
                        note.push_str("  · best copy");
                    } else if let Some(song) = app.song(&dialog.ids[index]) {
                        if (song.duration - longest).abs() > 10.0 {
                            note.push_str("  · a different length: maybe another version");
                        }
                    }
                    ui.label(RichText::new(note).font(regular(12.0)).color(t.muted));
                });
            });
            ui.add_space(4.0);
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let count = dialog.remove.iter().filter(|r| **r).count();
            let keeps_one = dialog.remove.iter().any(|r| !*r);
            let button = egui::Button::new(RichText::new(format!("Delete {}", plural(count, "copy").replace("copys", "copies"))).font(semibold(14.0)).color(Color32::WHITE))
                .fill(t.danger).corner_radius(CornerRadius::same(18)).min_size(vec2(0.0, 36.0));
            remove = ui.add_enabled(count > 0 && keeps_one, button).on_disabled_hover_text("Keep at least one copy").clicked();
            cancel = plain_button(ui, t, "Cancel");
        });
    });
    if remove {
        app.duplicates = Some(dialog);
        app.remove_duplicates();
    } else if !cancel && !ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.duplicates = Some(dialog);
    }
}

// Albums and genres from Wikipedia: what it has that the songs don't. Missing names are
// ticked; different ones are offered unticked. Names you set yourself aren't touched.
fn wiki_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    if !app.wiki_open {
        return;
    }
    let (mut apply, mut close) = (false, false);
    let rows: Vec<(String, String, String, String, String)> = app.wiki_list.iter().map(|item| {
        let song = app.song(&item.id);
        let (title, artist, album, genre) = song.map_or(Default::default(), |s| (s.title.clone(), s.artist.clone(), s.album.clone(), s.genre.clone()));
        (item.id.clone(), title, artist, album, genre)
    }).collect();
    egui::Modal::new(egui::Id::new("wiki")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(640.0);
        ui.label(RichText::new("Albums and genres from Wikipedia").font(bold(20.0)));
        let status = if app.wiki_offline {
            "Wikipedia can't be reached right now; what's here was found earlier.".to_string()
        } else if app.wiki_waiting > 0 {
            format!("Still looking up {} in the background; more may show up.", plural(app.wiki_waiting, "song"))
        } else {
            "Every song has been looked up.".to_string()
        };
        ui.label(RichText::new(status).font(regular(12.5)).color(t.muted));
        ui.add_space(8.0);
        if rows.is_empty() {
            ui.label(RichText::new("Your songs already have what Wikipedia knows about their albums and genres.").color(t.muted));
        } else {
            ui.label(RichText::new("Missing albums and genres are ticked; ones that differ from yours are offered unticked.").color(t.muted));
            ui.add_space(6.0);
            let height = (rows.len() as f32 * 74.0).min(420.0);
            egui::ScrollArea::vertical().max_height(420.0).min_scrolled_height(height).show(ui, |ui| {
                for (index, (id, title, artist, album, genre)) in rows.iter().enumerate() {
                    let item = &app.wiki_list[index];
                    ui.label(RichText::new(format!("{title} · {artist}")).font(semibold(13.5)));
                    if let Some((wiki_album, _)) = &item.album {
                        let tick = app.wiki_ticks.entry((id.clone(), false)).or_insert(false);
                        let now = if album.is_empty() { String::new() } else { format!("   (now: {album})") };
                        ui.checkbox(tick, RichText::new(format!("Album: {wiki_album}{now}")).font(regular(13.0)));
                    }
                    if let Some((wiki_genres, _)) = &item.genres {
                        let tick = app.wiki_ticks.entry((id.clone(), true)).or_insert(false);
                        let now = if genre.is_empty() { String::new() } else { format!("   (now: {genre})") };
                        ui.checkbox(tick, RichText::new(format!("Genres: {}{now}", wiki_genres.join(", "))).font(regular(13.0)));
                    }
                    ui.add_space(6.0);
                }
            });
        }
        ui.add_space(12.0);
        ui.horizontal(|ui| {
            let count = app.wiki_list.iter().filter(|item| {
                app.wiki_ticks.get(&(item.id.clone(), false)).copied().unwrap_or(false) && item.album.is_some()
                    || app.wiki_ticks.get(&(item.id.clone(), true)).copied().unwrap_or(false) && item.genres.is_some()
            }).count();
            if !rows.is_empty() {
                apply = primary_button(ui, t, &format!("Use for {}", plural(count, "song")), count > 0);
            }
            close = plain_button(ui, t, if rows.is_empty() { "Close" } else { "Cancel" });
        });
    });
    if apply {
        app.apply_wiki();
        app.wiki_open = false;
    } else if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.wiki_open = false;
    }
}

// ---------- Downloads ----------

fn download_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    if !app.downloads.open {
        return;
    }
    let mut open = true;
    let screen = ctx.content_rect();
    egui::Window::new(RichText::new("Download songs").font(bold(18.0)))
        .open(&mut open)
        .collapsible(false)
        .resizable(false)
        .default_pos(pos2(screen.center().x - 300.0, 120.0))
        .frame(window_frame(ctx, t))
        .show(ctx, |ui| {
            ui.set_width(600.0);
            let tools = app.downloads.tools.clone();
            if !tools.ready() {
                ui.label(RichText::new("Downloads need three free programs: yt-dlp (finds the music), ffmpeg (makes mp3s) and deno (reads YouTube's pages). \
                    They're downloaded once into the player's folder (about 250 MB).").color(t.muted));
                ui.add_space(8.0);
                if app.downloads.getting_tools {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Getting them… (this takes a minute or two)");
                    });
                } else if primary_button(ui, t, "Get them", true) {
                    app.get_download_tools();
                }
            } else {
                ui.label(RichText::new("Paste a link to a song or a playlist (YouTube, SoundCloud, Bandcamp and many more). Songs are saved into the open folder as \"Artist - Title.mp3\".").color(t.muted));
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let field = egui::TextEdit::singleline(&mut app.downloads.link).hint_text("https://…").desired_width(470.0).font(regular(14.5)).margin(vec2(10.0, 8.0));
                    let response = ui.add(field);
                    let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let label = if app.downloads.checking { "Checking…" } else { "Check" };
                    if (primary_button(ui, t, label, !app.downloads.checking && !app.downloads.link.trim().is_empty()) || enter) && !app.downloads.checking {
                        app.check_link();
                    }
                });
            }
            if let Some(error) = &app.downloads.error {
                ui.add_space(6.0);
                ui.label(RichText::new(error).color(t.danger));
            }

            // What the link holds: names can be fixed before downloading
            if !app.downloads.entries.is_empty() {
                ui.add_space(8.0);
                if !app.downloads.playlist.is_empty() {
                    ui.label(RichText::new(format!("{} · {}", app.downloads.playlist, plural(app.downloads.entries.len(), "song"))).font(semibold(14.0)));
                }
                ui.label(RichText::new("Check the names: they become the file names and the songs' names.").font(regular(12.5)).color(t.muted));
                let height = (app.downloads.entries.len() as f32 * 40.0).min(300.0);
                egui::ScrollArea::vertical().id_salt("entries").max_height(300.0).min_scrolled_height(height).show(ui, |ui| {
                    for entry in app.downloads.entries.iter_mut() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut entry.chosen, "");
                            ui.add(egui::TextEdit::singleline(&mut entry.artist).hint_text("Artist").desired_width(200.0));
                            ui.label(RichText::new("–").color(t.muted));
                            ui.add(egui::TextEdit::singleline(&mut entry.title).hint_text("Title").desired_width(300.0));
                        });
                    }
                });
                ui.add_space(8.0);
                let count = app.downloads.entries.iter().filter(|e| e.chosen).count();
                if primary_button(ui, t, &format!("Download {}", plural(count, "song")), count > 0) {
                    app.start_downloads();
                }
            }

            // The downloads, and how each is going
            if !app.downloads.jobs.is_empty() {
                ui.add_space(10.0);
                ui.separator();
                let height = (app.downloads.jobs.len() as f32 * 26.0).min(240.0);
                egui::ScrollArea::vertical().id_salt("jobs").max_height(240.0).min_scrolled_height(height).show(ui, |ui| {
                    use crate::downloads::State;
                    for (entry, state) in &app.downloads.jobs {
                        ui.horizontal(|ui| {
                            let name = if entry.artist.is_empty() { entry.title.clone() } else { format!("{} – {}", entry.artist, entry.title) };
                            ui.add_sized([360.0, 20.0], egui::Label::new(RichText::new(name).font(regular(13.0))).truncate());
                            let (text, color) = match state {
                                State::Waiting => ("waiting".to_string(), t.muted),
                                State::Downloading(percent) => (format!("{percent:.0}%"), t.accent),
                                State::Converting => ("making the mp3…".to_string(), t.accent),
                                State::Done(_) => ("done".to_string(), t.accent),
                                State::Failed(reason) => (format!("failed: {reason}"), t.danger),
                                State::Cancelled => ("cancelled".to_string(), t.muted),
                            };
                            if let State::Downloading(percent) = state {
                                let (rect, _) = ui.allocate_exact_size(vec2(120.0, 6.0), Sense::hover());
                                ui.painter().rect_filled(rect, 3.0, t.track);
                                gradient_rect(ui.painter(), Rect::from_min_size(rect.min, vec2(rect.width() * percent / 100.0, rect.height())), t.accent, t.accent2);
                            }
                            ui.label(RichText::new(text).font(regular(12.5)).color(color));
                        });
                    }
                });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    if app.downloads.active() > 0 {
                        if plain_button(ui, t, "Cancel all") {
                            app.cancel_downloads();
                        }
                    } else if plain_button(ui, t, "Clear the list") {
                        app.clear_finished_downloads();
                    }
                });
            }
        });
    if !open {
        app.downloads.open = false; // (downloads go on in the background)
    }
}

// Importing replaces the library: asked first
fn import_window(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some(from) = app.import_pending.clone() else { return };
    let (mut import, mut cancel) = (false, false);
    egui::Modal::new(egui::Id::new("import")).frame(window_frame(ctx, t)).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.label(RichText::new("Import this library?").font(bold(20.0)));
        ui.add_space(6.0);
        ui.label(RichText::new(format!(
            "{} replaces your library here: names, albums, genres, hearts, plays, covers, lyrics and settings. \
             What's here now is kept as a backup (in the player's \"backups\" folder). Your music files aren't touched.",
            from.file_name().map_or(String::new(), |n| n.to_string_lossy().to_string())
        )).color(t.muted));
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            import = primary_button(ui, t, "Import", true);
            cancel = plain_button(ui, t, "Cancel");
        });
    });
    if import {
        app.import_pending = None;
        app.import_library(&from);
    } else if cancel || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.import_pending = None;
    }
}

// ---------- Messages ----------

fn toast(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some((message, since)) = &app.toast else { return };
    let shown_for = if message.len() > 90 { 7.0 } else { 4.0 };
    let age = since.elapsed().as_secs_f32();
    if age > shown_for {
        app.toast = None;
        return;
    }
    let fade = ((shown_for - age) / 0.4).clamp(0.0, 1.0);
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("toast"))
        .order(egui::Order::Tooltip)
        .pivot(Align2::CENTER_BOTTOM)
        .fixed_pos(pos2(screen.center().x, screen.max.y - 112.0))
        .interactable(false)
        .show(ctx, |ui| {
            ui.set_opacity(fade);
            Frame::popup(ui.style()).fill(t.menu).corner_radius(CornerRadius::same(14)).inner_margin(Margin::symmetric(16, 10)).show(ui, |ui| {
                ui.set_max_width(560.0);
                ui.label(RichText::new(message).font(regular(14.0)));
            });
        });
    ctx.request_repaint_after(Duration::from_millis(100));
}

// The very first start: bring the library from the old player, or start fresh
fn welcome(app: &mut App, ctx: &egui::Context, t: &Theme) {
    let Some(welcome) = &app.welcome else { return };
    let message = welcome.message.clone();
    let mut choice = None;
    let mut zip: Option<std::path::PathBuf> = None;
    let frame = Frame::popup(&ctx.style()).fill(t.menu).corner_radius(CornerRadius::same(20)).inner_margin(Margin::same(28));
    egui::Modal::new(egui::Id::new("welcome")).frame(frame).backdrop_color(Color32::from_black_alpha(90)).show(ctx, |ui| {
        {
            ui.set_width(460.0);
            ui.label(RichText::new(icon::MUSIC_NOTES).font(icons(34.0)).color(t.accent));
            ui.add_space(6.0);
            ui.label(RichText::new("Welcome to Music Player").font(bold(22.0)));
            ui.add_space(8.0);
            ui.label(RichText::new(
                "Have a library already? It can come along: hearts, plays, the names you fixed, covers and saved lyrics. \
                 Pick its folder, the one with library.json in it: the old player's folder, or a Music Player data folder \
                 copied from another computer (~/.local/share/music-player there).",
            ).color(t.muted));
            ui.add_space(4.0);
            ui.label(RichText::new("No library yet? Start fresh: the player opens your Music folder.").color(t.muted));
            if let Some(message) = &message {
                ui.add_space(8.0);
                ui.label(RichText::new(message).color(t.danger));
            }
            ui.add_space(18.0);
            ui.horizontal(|ui| {
                let bring = egui::Button::new(with_icon(icon::FOLDER_OPEN, "Use an existing library", semibold(14.0), t.on_accent))
                    .fill(t.accent)
                    .corner_radius(CornerRadius::same(18))
                    .min_size(vec2(0.0, 38.0));
                if ui.add(bring).clicked() {
                    choice = Some(true);
                }
                let fresh = egui::Button::new(RichText::new("Start fresh").font(semibold(14.0)))
                    .fill(t.hover)
                    .corner_radius(CornerRadius::same(18))
                    .min_size(vec2(0.0, 38.0));
                if ui.add(fresh).clicked() {
                    choice = Some(false);
                }
            });
            ui.add_space(6.0);
            if ui.add(egui::Button::new(with_icon(icon::FILE_ZIP, "Or import a .zip made with Export", regular(13.0), t.accent)).frame(false)).clicked() {
                zip = rfd::FileDialog::new().set_title("Import a library").add_filter("Zip", &["zip"]).pick_file();
            }
        }
    });
    if let Some(zip) = zip {
        app.import_library(&zip);
        return;
    }
    match choice {
        Some(true) => app.import_old_library(),
        Some(false) => app.start_fresh(),
        None => {}
    }
}
