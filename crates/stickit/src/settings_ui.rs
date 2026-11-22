//! `stickit --settings`: a window for stickit.ini. Every control writes the file as you change
//! it, and the running app picks the change up within a second, as it does for edits by hand.
//! It runs as its own process (the app starts it from "Settings..."), so the notes' event loop
//! and drawing are left alone.

use crate::config::{Config, Kind};
use eframe::egui::{self, Color32, RichText};
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Pause after the last change before the file is written (a dragged slider settles first).
const SAVE_AFTER: Duration = Duration::from_millis(200);
/// How often the file is checked for changes made elsewhere (by hand, or the app itself).
const CHECK_EVERY: Duration = Duration::from_secs(1);
/// macOS takes the focus away some moment after the Dock icon is hidden, and ignores asking for
/// it back until then: so for this long, it is asked for again whenever it is found gone.
const REFOCUS_FOR: Duration = Duration::from_millis(1200);
const REFOCUS_EVERY: Duration = Duration::from_millis(150);

/// The first of these that exists is the window's text font: egui's own (Ubuntu Light) is too
/// thin to read comfortably at these sizes. egui's fonts stay behind it for symbols such as ↺.
const SYSTEM_FONTS: [(&str, u32); 4] = [
    ("/System/Library/Fonts/SFNS.ttf", 0),
    ("/System/Library/Fonts/HelveticaNeue.ttc", 0),
    ("C:\\Windows\\Fonts\\segoeui.ttf", 0),
    ("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf", 0),
];

fn use_system_font(ctx: &egui::Context) {
    let Some((bytes, index)) = SYSTEM_FONTS.iter().find_map(|(p, i)| std::fs::read(p).ok().map(|b| (b, *i))) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    let mut data = egui::FontData::from_owned(bytes);
    data.index = index;
    fonts.font_data.insert("system".to_owned(), std::sync::Arc::new(data));
    if let Some(family) = fonts.families.get_mut(&egui::FontFamily::Proportional) {
        family.insert(0, "system".to_owned());
    }
    ctx.set_fonts(fonts);
}

/// The zoom (⌘+ / ⌘−) is kept in this file beside stickit.ini, so the window opens at it again.
fn zoom_file(config: &std::path::Path) -> PathBuf {
    config.with_file_name("settings-zoom")
}

fn saved_zoom(config: &std::path::Path) -> f32 {
    std::fs::read_to_string(zoom_file(config))
        .ok()
        .and_then(|s| s.trim().parse::<f32>().ok())
        .filter(|z| z.is_finite())
        .map_or(1.0, |z| z.clamp(0.5, 3.0))
}

pub fn run(path: PathBuf) {
    let zoom = saved_zoom(&path);
    let app = SettingsApp::new(path);
    let show_icon = app.dock_icon;
    let options = eframe::NativeOptions {
        // This window is a process of its own, so it would get a Dock icon of its own.
        event_loop_builder: Some(Box::new(move |builder| crate::dock_icon::at_launch(builder, show_icon))),
        viewport: egui::ViewportBuilder::default()
            .with_title("Stick-itt Settings")
            .with_inner_size([780.0 * zoom, 600.0 * zoom])
            .with_min_inner_size([560.0, 380.0]),
        ..Default::default()
    };
    let _ = eframe::run_native(
        "Stick-itt Settings",
        options,
        Box::new(move |cc| {
            use_system_font(&cc.egui_ctx);
            // Both themes, so switching between them keeps the sizes; text darker (or lighter)
            // than egui's grey, for contrast.
            cc.egui_ctx.all_styles_mut(|style| {
                for (text, size) in [
                    (egui::TextStyle::Body, 14.0),
                    (egui::TextStyle::Button, 14.0),
                    (egui::TextStyle::Small, 12.0),
                    (egui::TextStyle::Heading, 21.0),
                    (egui::TextStyle::Monospace, 13.0),
                ] {
                    if let Some(font) = style.text_styles.get_mut(&text) {
                        font.size = size;
                    }
                }
                style.spacing.item_spacing = egui::vec2(10.0, 8.0);
                style.spacing.slider_width = 190.0;
                style.spacing.button_padding = egui::vec2(8.0, 4.0);
                let dark = style.visuals.dark_mode;
                style.visuals.widgets.noninteractive.fg_stroke.color =
                    if dark { Color32::from_gray(215) } else { Color32::from_gray(25) };
                style.visuals.weak_text_alpha = 0.72;
            });
            cc.egui_ctx.set_zoom_factor(app.zoom);
            Ok(Box::new(app))
        }),
    );
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Section(usize),
    Shortcuts,
}

struct SettingsApp {
    cfg: Config,
    page: Page,
    /// Text being typed into a field, by "section.key"; applied when the field is left.
    drafts: HashMap<String, String>,
    /// Shortcut fields that hold an unreadable chord.
    invalid: HashMap<String, bool>,
    /// The shortcut waiting for a key press ("global.key" or "note.key").
    recording: Option<String>,
    changed_at: Option<Instant>,
    checked_at: Instant,
    /// Whether this process shows a Dock icon now.
    dock_icon: bool,
    /// Until when to take the focus back, after hiding the Dock icon.
    refocus_until: Option<Instant>,
    /// The zoom last saved to `zoom_file`.
    zoom: f32,
}

impl SettingsApp {
    fn new(path: PathBuf) -> Self {
        let zoom = saved_zoom(&path);
        let cfg = Config::load(Some(path));
        SettingsApp {
            zoom,
            dock_icon: cfg.general.dock_icon,
            refocus_until: None,
            cfg,
            page: Page::Section(0),
            drafts: HashMap::new(),
            invalid: HashMap::new(),
            recording: None,
            changed_at: None,
            checked_at: Instant::now(),
        }
    }

    fn changed(&mut self) {
        self.changed_at = Some(Instant::now());
    }

    fn flush(&mut self) {
        if self.changed_at.take().is_some() {
            self.cfg.save();
        }
    }

    fn sync_file(&mut self, ctx: &egui::Context) {
        if let Some(at) = self.changed_at {
            if at.elapsed() >= SAVE_AFTER {
                self.flush();
            } else {
                ctx.request_repaint_after(SAVE_AFTER);
            }
        }
        // Changes from elsewhere (e.g. Ctrl+wheel over the dock) show up here, unless ours are
        // still waiting to be written.
        if self.changed_at.is_none() && self.checked_at.elapsed() >= CHECK_EVERY {
            self.checked_at = Instant::now();
            if self.cfg.reload_if_changed() {
                self.drafts.clear();
            }
        }
        ctx.request_repaint_after(CHECK_EVERY);
    }

    fn follow_theme(&self, ctx: &egui::Context) {
        let theme = self
            .cfg
            .settings_in("general")
            .into_iter()
            .find(|s| s.0 == "theme")
            .map(|s| s.3)
            .unwrap_or_default();
        let pref = match theme.as_str() {
            "light" => egui::ThemePreference::Light,
            "dark" => egui::ThemePreference::Dark,
            _ => egui::ThemePreference::System,
        };
        if ctx.options(|o| o.theme_preference) != pref {
            ctx.set_theme(pref);
        }
    }

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        // The footer first, from the bottom up; the pages scroll in what is left above it, so
        // nothing overlaps when the window is short or zoomed in.
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            ui.add_space(12.0);
            if ui.button("Open stickit.ini").on_hover_text("Edit the settings file by hand").clicked() {
                self.flush();
                self.cfg.open_in_editor();
            }
            ui.label(RichText::new("Changes apply as you make them.").small().weak());
            ui.add_space(4.0);
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.pages(ui));
            });
        });
    }

    fn pages(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.label(RichText::new("Stick-itt").heading().strong());
        ui.label(RichText::new("Settings").weak());
        ui.add_space(12.0);
        for (i, (section, _)) in Config::sections().iter().enumerate() {
            let label = section_title(section);
            if ui.add_sized([ui.available_width(), 26.0], egui::Button::selectable(self.page == Page::Section(i), label)).clicked() {
                self.page = Page::Section(i);
            }
        }
        let shortcuts = self.page == Page::Shortcuts;
        if ui.add_sized([ui.available_width(), 26.0], egui::Button::selectable(shortcuts, "Shortcuts")).clicked() {
            self.page = Page::Shortcuts;
        }
    }

    fn section_page(&mut self, ui: &mut egui::Ui, index: usize) {
        let (section, about) = Config::sections()[index];
        ui.heading(section_title(section));
        ui.label(RichText::new(about).weak());
        ui.add_space(10.0);
        let rows = self.cfg.settings_in(section);
        egui::Grid::new(section).num_columns(2).spacing([28.0, 16.0]).min_col_width(220.0).show(ui, |ui| {
            for (key, kind, doc, value, default) in rows {
                describe(ui, &humanize(key), doc);
                ui.horizontal(|ui| {
                    if let Some(v) = self.control(ui, section, key, kind, &value) {
                        self.cfg.set_value(section, key, &v);
                        self.changed();
                    }
                    if value != default
                        && ui.small_button("↺").on_hover_text(format!("Back to the default: {default}")).clicked()
                    {
                        self.cfg.set_value(section, key, &default);
                        self.drafts.remove(&format!("{section}.{key}"));
                        self.changed();
                    }
                });
                ui.end_row();
            }
        });
    }

    /// Draws the control for one setting; returns the new value when it was changed.
    fn control(&mut self, ui: &mut egui::Ui, section: &str, key: &str, kind: Kind, value: &str) -> Option<String> {
        let id = format!("{section}.{key}");
        match kind {
            Kind::Num { lo, hi } => {
                let mut v: f32 = value.parse().unwrap_or(lo);
                let step = if hi - lo >= 100.0 {
                    1.0
                } else if hi - lo >= 10.0 {
                    0.5
                } else {
                    0.01
                };
                let slider = egui::Slider::new(&mut v, lo..=hi).step_by(step as f64).clamping(egui::SliderClamping::Always);
                ui.add(slider).changed().then(|| {
                    let s = format!("{:.3}", v);
                    s.trim_end_matches('0').trim_end_matches('.').to_string()
                })
            }
            Kind::Flag => {
                let mut on = value == "true";
                let label = if on { "On" } else { "Off" };
                ui.checkbox(&mut on, label).changed().then(|| on.to_string())
            }
            Kind::Choice(choices) => {
                let mut picked = None;
                egui::ComboBox::from_id_salt(&id).selected_text(humanize(value)).width(180.0).show_ui(ui, |ui| {
                    for c in choices {
                        if ui.selectable_label(value == *c, humanize(c)).clicked() {
                            picked = Some(c.to_string());
                        }
                    }
                });
                picked
            }
            Kind::NoteColor => {
                let shown = match value.parse::<u16>() {
                    Ok(hue) => format!("Hue {hue}°"),
                    Err(_) => humanize(value),
                };
                let mut picked = None;
                egui::ComboBox::from_id_salt(&id).selected_text(shown).width(180.0).height(320.0).show_ui(ui, |ui| {
                    for c in ["cycle", "random"] {
                        if ui.selectable_label(value == c, humanize(c)).clicked() {
                            picked = Some(c.to_string());
                        }
                    }
                    ui.separator();
                    for (i, name) in crate::theme::PRESET_NAMES.iter().enumerate() {
                        let (accent, _) = crate::theme::tint(i as u16, ui.visuals().dark_mode);
                        let swatch = RichText::new("●  ").color(Color32::from_rgb(accent.0, accent.1, accent.2));
                        let mut job = egui::text::LayoutJob::default();
                        swatch.append_to(&mut job, ui.style(), egui::FontSelection::Default, egui::Align::Center);
                        RichText::new(*name).append_to(&mut job, ui.style(), egui::FontSelection::Default, egui::Align::Center);
                        if ui.selectable_label(value.eq_ignore_ascii_case(name), job).clicked() {
                            picked = Some(name.to_lowercase());
                        }
                    }
                });
                picked
            }
            Kind::Color => {
                let auto = value == "auto";
                let mut out = None;
                let mut custom = !auto;
                if ui.checkbox(&mut custom, "Custom").changed() {
                    out = Some(if custom { "#4F8EF7".to_string() } else { "auto".to_string() });
                }
                if custom {
                    let hex = value.trim_start_matches('#');
                    let n = u32::from_str_radix(hex, 16).unwrap_or(0x4F8EF7);
                    let mut rgb = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
                    if ui.color_edit_button_srgb(&mut rgb).changed() {
                        out = Some(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]));
                    }
                } else {
                    ui.label(RichText::new("follows light / dark").weak());
                }
                out
            }
            Kind::Text => {
                let draft = self.drafts.entry(id.clone()).or_insert_with(|| value.to_string());
                let r = ui.add(egui::TextEdit::singleline(draft).desired_width(220.0));
                if r.lost_focus() {
                    let typed = self.drafts.remove(&id).unwrap_or_default();
                    // An unreadable list is ignored by the setting and the field shows the old value.
                    return (typed != value).then_some(typed);
                }
                None
            }
        }
    }

    fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        ui.heading("Shortcuts");
        ui.label(
            RichText::new("Click Record and press the keys, or type one like CmdOrCtrl+Alt+N. Leave it empty to turn it off.")
                .weak(),
        );
        self.capture(ui);
        for global in [true, false] {
            ui.add_space(12.0);
            ui.label(RichText::new(if global { "Anywhere" } else { "In a note" }).strong().size(15.0));
            ui.add_space(4.0);
            let rows = self.cfg.shortcuts(global);
            let grid = if global { "global" } else { "note" };
            egui::Grid::new(grid).num_columns(2).spacing([28.0, 10.0]).min_col_width(220.0).show(ui, |ui| {
                for (key, doc, binding, default) in rows {
                    describe(ui, &humanize(key), doc);
                    ui.horizontal(|ui| self.shortcut_row(ui, global, key, &binding, default));
                    ui.end_row();
                }
            });
        }
    }

    fn shortcut_row(&mut self, ui: &mut egui::Ui, global: bool, key: &str, binding: &str, default: &str) {
        let id = format!("{}.{key}", if global { "global" } else { "note" });
        let bad = self.invalid.get(&id).copied().unwrap_or(false);
        let draft = self.drafts.entry(id.clone()).or_insert_with(|| binding.to_string());
        let mut edit = egui::TextEdit::singleline(draft).desired_width(170.0).hint_text("off");
        if bad {
            edit = edit.text_color(ui.visuals().error_fg_color);
        }
        if ui.add(edit).lost_focus() {
            let typed = self.drafts.get(&id).cloned().unwrap_or_default();
            self.apply_shortcut(global, key, &id, &typed);
        }
        let recording = self.recording.as_deref() == Some(id.as_str());
        let label = if recording { "Press keys…" } else { "Record" };
        if ui.selectable_label(recording, label).clicked() {
            self.recording = if recording { None } else { Some(id.clone()) };
        }
        if binding != default && ui.small_button("↺").on_hover_text(format!("Back to the default: {default}")).clicked() {
            self.apply_shortcut(global, key, &id, default);
        }
        if bad {
            ui.label(RichText::new("not a shortcut").color(ui.visuals().error_fg_color).small());
        }
    }

    fn apply_shortcut(&mut self, global: bool, key: &str, id: &str, binding: &str) {
        if self.cfg.set_shortcut(global, key, binding) {
            self.invalid.remove(id);
            self.drafts.remove(id);
            self.changed();
        } else {
            self.invalid.insert(id.to_string(), true);
            self.drafts.insert(id.to_string(), binding.to_string());
        }
    }

    /// While a shortcut is being recorded, the next key press with its modifiers becomes it.
    fn capture(&mut self, ui: &mut egui::Ui) {
        let Some(id) = self.recording.clone() else { return };
        let pressed = ui.input(|i| {
            i.events.iter().find_map(|e| match e {
                egui::Event::Key { key, pressed: true, modifiers, .. } => Some((*key, *modifiers)),
                _ => None,
            })
        });
        let Some((key, mods)) = pressed else { return };
        if key == egui::Key::Escape && mods.is_none() {
            self.recording = None;
            return;
        }
        let Some(name) = key_name(key) else { return };
        let mut parts = Vec::new();
        if mods.mac_cmd {
            parts.push("Cmd");
        }
        if mods.ctrl {
            parts.push("Ctrl");
        }
        if mods.alt {
            parts.push("Alt");
        }
        if mods.shift {
            parts.push("Shift");
        }
        parts.push(&name);
        let chord = parts.join("+");
        let (global, key) = id.split_once('.').map(|(g, k)| (g == "global", k.to_string())).unwrap_or((true, String::new()));
        self.apply_shortcut(global, &key, &id, &chord);
        self.recording = None;
    }
}

impl eframe::App for SettingsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.follow_theme(ctx);
        egui::SidePanel::left("nav").resizable(false).exact_width(190.0).show(ctx, |ui| self.sidebar(ui));
        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
                ui.add_space(10.0);
                match self.page {
                    Page::Section(i) => self.section_page(ui, i),
                    Page::Shortcuts => self.shortcuts_page(ui),
                }
                ui.add_space(16.0);
            });
        });
        self.sync_file(ctx);
        self.follow_dock_icon(ctx);
        self.keep_zoom(ctx);
    }
}

impl SettingsApp {
    fn follow_dock_icon(&mut self, ctx: &egui::Context) {
        if self.dock_icon != self.cfg.general.dock_icon {
            self.dock_icon = self.cfg.general.dock_icon;
            let focused = ctx.input(|i| i.viewport().focused != Some(false));
            self.refocus_until = (focused && !self.dock_icon).then(|| Instant::now() + REFOCUS_FOR);
            crate::dock_icon::set(self.dock_icon);
        }
        if let Some(until) = self.refocus_until {
            if ctx.input(|i| i.viewport().focused == Some(false)) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
            if Instant::now() >= until {
                self.refocus_until = None;
            } else {
                ctx.request_repaint_after(REFOCUS_EVERY);
            }
        }
    }

    fn keep_zoom(&mut self, ctx: &egui::Context) {
        let zoom = ctx.zoom_factor();
        if (zoom - self.zoom).abs() > 0.001 {
            self.zoom = zoom;
            if let Some(path) = self.cfg.path() {
                let _ = std::fs::write(zoom_file(path), format!("{zoom:.2}\n"));
            }
        }
    }
}

impl Drop for SettingsApp {
    fn drop(&mut self) {
        self.flush();
    }
}

/// The left column of a row: the setting's name, with its explanation underneath.
fn describe(ui: &mut egui::Ui, name: &str, doc: &str) {
    ui.vertical(|ui| {
        ui.set_max_width(260.0);
        ui.label(RichText::new(name).strong());
        let doc = clean_doc(doc);
        if !doc.is_empty() {
            ui.label(RichText::new(doc).small().weak());
        }
    });
}

/// The file's comment, without the `#` that starts its continuation lines.
fn clean_doc(doc: &str) -> String {
    doc.lines().map(|l| l.trim().trim_start_matches('#').trim()).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

fn section_title(section: &str) -> String {
    match section {
        "notes.compact" => "Compact view".into(),
        "notes.fade" => "Fading".into(),
        "dock.list" => "Dock: list".into(),
        "dock.icons" => "Dock: icons".into(),
        "dock.tab" => "Dock: edge tab".into(),
        other => humanize(other),
    }
}

/// "open_delay_ms" → "Open delay ms".
fn humanize(key: &str) -> String {
    let words = key.replace(['_', '.'], " ");
    let mut c = words.chars();
    match c.next() {
        Some(first) => first.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// The settings file's name for a key (what `Chord::parse` reads), or None for keys it can't bind.
fn key_name(key: egui::Key) -> Option<String> {
    use egui::Key as K;
    let named = match key {
        K::ArrowUp => "ArrowUp",
        K::ArrowDown => "ArrowDown",
        K::ArrowLeft => "ArrowLeft",
        K::ArrowRight => "ArrowRight",
        K::Enter => "Enter",
        K::Tab => "Tab",
        K::Space => "Space",
        K::Escape => "Escape",
        K::Backspace => "Backspace",
        K::Delete => "Delete",
        K::Equals | K::Plus => "Equal",
        K::Minus => "Minus",
        K::Comma => "Comma",
        K::Period => "Period",
        K::Slash => "Slash",
        K::Backtick => "Backquote",
        K::OpenBracket => "BracketLeft",
        K::CloseBracket => "BracketRight",
        _ => {
            let n = key.name();
            let simple = n.len() == 1 && n.chars().all(|c| c.is_ascii_alphanumeric());
            let function = n.starts_with('F') && n[1..].parse::<u8>().is_ok();
            return (simple || function).then(|| n.to_string());
        }
    };
    Some(named.to_string())
}
