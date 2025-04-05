#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use eframe::{egui, App, NativeOptions};
use egui::{
    text::LayoutJob, Align, Color32, FontId, Frame, Image, Layout, Margin, RichText, Rounding,
    ScrollArea, Separator, Stroke, TextFormat, ViewportBuilder,
};
#[allow(deprecated)] // Allow RetainedImage for now
use egui_extras::RetainedImage;
use lazy_static::lazy_static;
use open; // Keep open
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use rfd::FileDialog;
use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Style as SyntectStyle, ThemeSet};
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
#[cfg(windows)]
use winreg::{enums::HKEY_CURRENT_USER, RegKey};

lazy_static! {
    static ref SYNTAX_SET: SyntaxSet = SyntaxSet::load_defaults_newlines();
    static ref THEME_SET: ThemeSet = ThemeSet::load_defaults();
    #[allow(deprecated)] // Allow RetainedImage for now
    static ref IMAGE_CACHE: Mutex<HashMap<String, RetainedImage>> = Mutex::new(HashMap::new());
}

const CODE_FONT_SIZE: f32 = 13.0;
const BODY_FONT_SIZE: f32 = 14.0;

#[cfg(windows)]
fn hide_console() {
    if !cfg!(debug_assertions) {
        use std::ptr;
        use winapi::um::wincon::GetConsoleWindow;
        use winapi::um::winuser::{ShowWindow, SW_HIDE};
        let console_wnd = unsafe { GetConsoleWindow() };
        if console_wnd != ptr::null_mut() {
            unsafe {
                ShowWindow(console_wnd, SW_HIDE);
            }
        }
    }
}

#[cfg(not(windows))]
fn hide_console() {}

#[derive(Default)]
struct MarkdownViewerApp {
    markdown: String,
    file_path: Option<PathBuf>,
    status_message: Option<(String, f64)>,
    dark_mode: bool,
    last_modified: Option<SystemTime>,
    scroll_offset: Option<f32>, // Store absolute Y offset
}

impl MarkdownViewerApp {
    fn new_from_file(path: PathBuf) -> Self {
        log::info!("Loading file: {}", path.display());
        match fs::metadata(&path) {
            Ok(metadata) => {
                let modified = metadata.modified().ok();
                match fs::read_to_string(&path) {
                    Ok(content) => Self {
                        markdown: content,
                        file_path: Some(path),
                        status_message: Some(("File loaded.".to_string(), current_time())),
                        dark_mode: true,
                        last_modified: modified,
                        scroll_offset: None, // Reset scroll on new file
                    },
                    Err(e) => {
                        log::error!("Failed to read file {}: {}", path.display(), e);
                        Self::error(format!("Failed to read file: {}", e))
                    }
                }
            }
            Err(e) => {
                log::error!("Failed to get metadata for file {}: {}", path.display(), e);
                Self::error(format!("Failed to access file metadata: {}", e))
            }
        }
    }

    fn new_default() -> Self {
        log::info!("Loading default content.");
        Self {
            markdown: String::from(DEFAULT_MARKDOWN),
            dark_mode: true,
            ..Default::default()
        }
    }

    fn error(message: String) -> Self {
        Self {
            status_message: Some((message, current_time())),
            dark_mode: true,
            ..Default::default()
        }
    }

    fn reload_file(&mut self) {
        if let Some(path) = self.file_path.clone() {
            log::info!("Reloading file: {}", path.display());
            let current_scroll = self.scroll_offset;
            *self = Self::new_from_file(path);
            self.scroll_offset = current_scroll;
            self.status_message = Some(("File reloaded.".to_string(), current_time()));
        } else {
            log::warn!("Reload called with no file path set.");
            self.status_message = Some((
                "Cannot reload: No file is open.".to_string(),
                current_time(),
            ));
        }
    }

    fn check_file_modified(&mut self, ctx: &egui::Context) {
        if let Some(path) = &self.file_path {
            if let Ok(metadata) = fs::metadata(path) {
                if let Ok(modified) = metadata.modified() {
                    if self.last_modified.is_some() && self.last_modified != Some(modified) {
                        self.status_message = Some((
                            "File modified externally. Click Reload (🔄) to update.".to_string(),
                            current_time() + 10.0,
                        ));
                        self.last_modified = Some(modified);
                        ctx.request_repaint();
                    } else if self.last_modified.is_none() {
                        self.last_modified = Some(modified);
                    }
                }
            } else {
                if self.status_message.is_none()
                    || !self
                        .status_message
                        .as_ref()
                        .unwrap()
                        .0
                        .contains("accessible")
                {
                    log::warn!("Could not get metadata for open file: {}", path.display());
                    self.status_message = Some((
                        format!(
                            "Warning: File '{}' is no longer accessible.",
                            path.file_name().map_or_else(
                                || path.display().to_string(),
                                |n| n.to_string_lossy().to_string()
                            )
                        ),
                        current_time() + 10.0,
                    ));
                    ctx.request_repaint();
                }
            }
        }
    }

    fn get_syntect_theme(&self, visuals: &egui::Visuals) -> &'static syntect::highlighting::Theme {
        let theme_name = if visuals.dark_mode {
            "base16-ocean.dark"
        } else {
            "base16-ocean.light"
        };
        THEME_SET.themes.get(theme_name).unwrap_or_else(|| {
            log::warn!("Syntax theme '{}' not found, falling back.", theme_name);
            if visuals.dark_mode {
                &THEME_SET.themes["base16-eighties.dark"]
            } else {
                &THEME_SET.themes["base16-ocean.light"]
            }
        })
    }
}

impl App for MarkdownViewerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Check for external file modifications every 60 frames.
        if ctx.frame_nr() % 60 == 0 {
            self.check_file_modified(ctx);
        }

        let visuals = if self.dark_mode {
            let mut v = egui::Visuals::dark();
            // Custom dark theme with blue-tinged gray
            let dark_bg = Color32::from_rgb(40, 40, 55);
            v.window_fill = dark_bg;
            v.panel_fill = dark_bg;
            v.faint_bg_color = Color32::from_rgb(50, 50, 65);
            v.widgets.noninteractive.bg_fill = dark_bg;

            // Set text to near-white (240,240,245 is a slightly blue-tinged white)
            v.widgets.noninteractive.fg_stroke.color = Color32::from_rgb(240, 240, 245);
            v.override_text_color = Some(Color32::from_rgb(240, 240, 245)); // Correct way to override text color

            v
        } else {
            let mut v = egui::Visuals::light();
            // Light theme adjustments
            v.code_bg_color = Color32::from_rgb(245, 245, 245);
            v.extreme_bg_color = Color32::from_rgb(255, 255, 255);
            v.window_fill = Color32::from_rgb(255, 255, 255);
            v.panel_fill = Color32::from_rgb(255, 255, 255);
            v.widgets.noninteractive.bg_fill = Color32::from_rgb(255, 255, 255);
            v.override_text_color = None; // Use default light mode text colors
            v
        };
        ctx.set_visuals(visuals.clone());

        // Obtain the syntect theme based on the current visuals.
        let syntect_theme = self.get_syntect_theme(&visuals);

        // Process any dropped files.
        handle_dropped_files(ctx, self);

        // --- Top Menu Bar ---
        egui::TopBottomPanel::top("menu_bar").show(ctx, |ui| {
            ui.horizontal(|ui| {
                if ui
                    .button("📂 Open")
                    .on_hover_text("Open a Markdown file")
                    .clicked()
                {
                    if let Some(file_path) = FileDialog::new()
                        .add_filter("Markdown", &["md", "markdown"])
                        .pick_file()
                    {
                        *self = Self::new_from_file(file_path);
                    }
                }
                ui.add_enabled_ui(self.file_path.is_some(), |ui| {
                    if ui
                        .button("🔄 Reload")
                        .on_hover_text("Reload the current file")
                        .clicked()
                    {
                        self.reload_file();
                    }
                });
                #[cfg(windows)]
                {
                    if ui
                        .button("📋 Register")
                        .on_hover_text("Register as default .md viewer (Windows only)")
                        .clicked()
                    {
                        match register_default_viewer() {
                            Ok(_) => {
                                log::info!("Successfully registered as default MD viewer.");
                                self.status_message = Some((
                                    "Registered as default MD viewer!".to_string(),
                                    current_time(),
                                ));
                            }
                            Err(e) => {
                                log::error!("Registration failed: {}", e);
                                self.status_message =
                                    Some((format!("Registration failed: {}", e), current_time()));
                            }
                        }
                    }
                }
                if ui
                    .toggle_value(&mut self.dark_mode, "🌙 Dark Mode")
                    .on_hover_text("Toggle Dark/Light Theme")
                    .clicked()
                {
                    // Mode toggled.
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if let Some(ref path) = self.file_path {
                        let filename = path
                            .file_name()
                            .map_or_else(|| path.to_string_lossy(), |n| n.to_string_lossy());
                        ui.label(RichText::new(filename).weak())
                            .on_hover_text(path.display().to_string());
                    } else {
                        ui.label(RichText::new("No file loaded").weak());
                    }
                });
            });
        });

        // --- Bottom Status Bar ---
        let mut clear_status = false;
        if let Some((message, expiry_time)) = self.status_message.as_ref() {
            let current_time_val = current_time();
            if current_time_val < *expiry_time {
                let msg_clone = message.clone();
                egui::TopBottomPanel::bottom("status_bar")
                    .frame(Frame::default().inner_margin(Margin::symmetric(4.0, 2.0)))
                    .show(ctx, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(&msg_clone);
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("✕").on_hover_text("Dismiss message").clicked() {
                                    clear_status = true;
                                }
                            });
                        });
                    });
            } else {
                clear_status = true;
            }
        }
        if clear_status {
            self.status_message = None;
        }

        // --- Central Panel for Markdown Rendering ---
        egui::CentralPanel::default()
            .frame(Frame {
                inner_margin: Margin::same(12.0),
                fill: visuals.window_fill, // Use window_fill color
                ..Default::default()
            })
            .show(ctx, |ui| {
                let scroll_id = ui.id().with("markdown_scroll");
                let mut remembered_offset = self.scroll_offset.take();
                let mut scroll_area = ScrollArea::vertical()
                    .id_source(scroll_id)
                    .auto_shrink([false, false]);
                if let Some(offset) = remembered_offset.take() {
                    scroll_area = scroll_area.vertical_scroll_offset(offset);
                }
                let scroll_output = scroll_area.show(ui, |ui| {
                    render_markdown(ui, &self.markdown, &visuals, syntect_theme);
                });
                self.scroll_offset = Some(scroll_output.state.offset.y);
            });

        // Build a title string (setting window title at runtime is not supported in eframe 0.27.2)
        let _title = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_name())
            .map(|name| format!("{} - Markdown Viewer", name.to_string_lossy()))
            .unwrap_or_else(|| "Markdown Viewer".to_string());
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        eframe::set_value(storage, "dark_mode", &self.dark_mode);
        log::info!("Saving state.");
    }
}

struct RenderState<'a, 'b> {
    inline_style_stack: Vec<TextFormat>,
    block_stack: Vec<BlockInfo>,
    current_job: LayoutJob,
    base_format: TextFormat,
    list_item_number: Option<u64>,
    in_table_header: bool,
    table_rows: Vec<Vec<LayoutJob>>,
    current_cell_job: LayoutJob,
    highlighter: Option<HighlightLines<'a>>,
    code_language: Option<String>,
    code_block_content: String,
    current_link_url: Option<String>,
    current_link_text: String,
    ui: &'b mut egui::Ui,
    visuals: &'b egui::Visuals,
    syntect_theme: &'a syntect::highlighting::Theme,
}

#[derive(Clone, Debug)]
enum BlockInfo {
    List(Option<u64>),
    BlockQuote,
    Table,
}

fn render_markdown<'a>(
    ui: &mut egui::Ui,
    markdown: &str,
    visuals: &egui::Visuals,
    syntect_theme: &'a syntect::highlighting::Theme,
) {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_FOOTNOTES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS;
    let parser = Parser::new_ext(markdown, options);
    let base_font_id = FontId::new(BODY_FONT_SIZE, egui::FontFamily::Proportional);
    let base_format = TextFormat {
        font_id: base_font_id.clone(),
        color: visuals.text_color(),
        background: Color32::TRANSPARENT, // Remove text backgrounds
        ..Default::default()
    };
    let mut state = RenderState {
        inline_style_stack: vec![base_format.clone()],
        block_stack: Vec::new(),
        current_job: LayoutJob::default(),
        base_format,
        list_item_number: None,
        in_table_header: false,
        table_rows: Vec::new(),
        current_cell_job: LayoutJob::default(),
        highlighter: None,
        code_language: None,
        code_block_content: String::new(),
        current_link_url: None,
        current_link_text: String::new(),
        ui,
        visuals,
        syntect_theme,
    };

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {
                    flush_block_content(&mut state);
                }
                Tag::Heading { level, .. } => {
                    flush_block_content(&mut state);
                    state.ui.add_space(heading_spacing(level, true));
                    let mut format = state.base_format.clone();
                    format.font_id = heading_font_id(level);
                    state.inline_style_stack.push(format);
                }
                Tag::BlockQuote => {
                    flush_block_content(&mut state);
                    state.ui.add_space(4.0);
                    state.block_stack.push(BlockInfo::BlockQuote);
                    state.ui.group(|ui| {
                        let style = ui.style_mut();
                        style.visuals.widgets.noninteractive.bg_fill = Color32::TRANSPARENT;
                        style.visuals.widgets.noninteractive.bg_stroke = Stroke::NONE;
                        Frame::none()
                            .inner_margin(Margin {
                                left: 10.0,
                                right: 4.0,
                                top: 2.0,
                                bottom: 2.0,
                            })
                            .show(ui, |ui| {
                                let rect = ui.available_rect_before_wrap();
                                let line_rect = egui::Rect::from_min_max(
                                    rect.left_top() + egui::vec2(2.0, 0.0),
                                    rect.left_bottom() + egui::vec2(4.0, 0.0),
                                );
                                ui.painter().rect_filled(
                                    line_rect,
                                    Rounding::ZERO,
                                    ui.visuals().widgets.noninteractive.fg_stroke.color,
                                );
                            });
                    });
                }
                Tag::CodeBlock(kind) => {
                    flush_block_content(&mut state);
                    state.ui.add_space(4.0);
                    state.code_language = match kind {
                        CodeBlockKind::Fenced(lang) if !lang.is_empty() => Some(lang.into_string()),
                        _ => None,
                    };
                    state.code_block_content.clear();
                    let syntax = state
                        .code_language
                        .as_deref()
                        .and_then(|lang| SYNTAX_SET.find_syntax_by_token(lang))
                        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text());
                    state.highlighter = Some(HighlightLines::new(syntax, state.syntect_theme));
                }
                Tag::List(start_num) => {
                    flush_block_content(&mut state);
                    state.ui.add_space(4.0);
                    state.block_stack.push(BlockInfo::List(start_num));
                    state.list_item_number = start_num;
                }
                Tag::Item => {
                    flush_block_content(&mut state);
                    let indent_level = state
                        .block_stack
                        .iter()
                        .filter(|b| matches!(b, BlockInfo::List(_)))
                        .count();
                    let indent_width = 20.0;
                    state.ui.indent("list_item_indent", |ui| {
                        ui.add_space((indent_level.saturating_sub(1)) as f32 * indent_width);
                        ui.horizontal_top(|ui| {
                            ui.set_width(indent_width);
                            let marker = match state.list_item_number {
                                Some(num) => format!("{}. ", num),
                                None => "• ".to_string(),
                            };
                            ui.label(RichText::new(marker).font(state.base_format.font_id.clone()));
                        });
                        if let Some(num) = state.list_item_number.as_mut() {
                            *num += 1;
                        }
                    });
                    state.ui.add_space(2.0);
                }
                Tag::Table(_alignments) => {
                    flush_block_content(&mut state);
                    state.ui.add_space(6.0);
                    state.block_stack.push(BlockInfo::Table);
                    state.table_rows.clear();
                }
                Tag::TableHead => {
                    state.in_table_header = true;
                    state.table_rows.push(Vec::new());
                    state.current_cell_job = LayoutJob::default();
                }
                Tag::TableRow => {
                    state.in_table_header = false;
                    state.table_rows.push(Vec::new());
                    state.current_cell_job = LayoutJob::default();
                }
                Tag::TableCell => {
                    state.current_cell_job = LayoutJob::default();
                    if state.in_table_header {
                        let mut format = state
                            .inline_style_stack
                            .last()
                            .cloned()
                            .unwrap_or_else(|| state.base_format.clone());
                        format.font_id =
                            FontId::new(format.font_id.size, egui::FontFamily::Proportional);
                        state.inline_style_stack.push(format);
                    }
                }
                Tag::Emphasis => {
                    let mut format = state.inline_style_stack.last().unwrap().clone();
                    format.italics = true;
                    state.inline_style_stack.push(format);
                }
                Tag::Strong => {
                    let mut format = state.inline_style_stack.last().unwrap().clone();
                    format.font_id =
                        FontId::new(format.font_id.size, egui::FontFamily::Proportional);
                    state.inline_style_stack.push(format);
                }
                Tag::Strikethrough => {
                    let mut format = state.inline_style_stack.last().unwrap().clone();
                    format.strikethrough = Stroke::new(1.0, format.color);
                    state.inline_style_stack.push(format);
                }
                Tag::Link {
                    link_type: _,
                    dest_url,
                    title: _,
                    id: _,
                } => {
                    flush_inline_content(&mut state, false); // Flush text before link
                    let mut format = state.inline_style_stack.last().unwrap().clone();
                    format.color = state.visuals.hyperlink_color;
                    format.underline = Stroke::new(1.0, format.color);
                    state.inline_style_stack.push(format);
                    state.current_link_url = Some(dest_url.into_string());
                    state.current_link_text.clear();
                }
                Tag::Image {
                    link_type: _,
                    dest_url,
                    title,
                    id: _,
                } => {
                    flush_inline_content(&mut state, false);
                    render_image(state.ui, dest_url.as_ref(), title.as_ref());
                }
                Tag::FootnoteDefinition(label) => {
                    flush_block_content(&mut state);
                    state.ui.add_space(4.0);
                    state.ui.label(format!("[^{}]:", label));
                    state.ui.indent("footnote", |ui| {});
                }
                _ => {}
            },
            Event::End(tag_end) => match tag_end {
                TagEnd::Paragraph => {
                    flush_inline_content(&mut state, true);
                }
                TagEnd::Heading(level) => {
                    // Corrected: tuple variant TagEnd::Heading(level)
                    if !state.inline_style_stack.is_empty() {
                        state.inline_style_stack.pop();
                    }
                    flush_inline_content(&mut state, false);
                    state.ui.add_space(heading_spacing(level, false));
                }
                TagEnd::BlockQuote => {
                    flush_block_content(&mut state);
                    if !state.block_stack.is_empty() {
                        state.block_stack.pop();
                    }
                    state.ui.add_space(6.0);
                }
                TagEnd::CodeBlock => {
                    render_code_block(&mut state);
                    state.highlighter = None;
                    state.code_language = None;
                    state.ui.add_space(6.0);
                }
                TagEnd::List(_) => {
                    // Keep List(_) as pulldown_cmark uses TagEnd::List(Option<u64>)
                    flush_block_content(&mut state);
                    if !state.block_stack.is_empty() {
                        state.block_stack.pop();
                    }
                    state.list_item_number = get_parent_list_num(&state.block_stack);
                    state.ui.add_space(6.0);
                }
                TagEnd::Item => {
                    // Keep Item as pulldown_cmark uses TagEnd::Item
                    flush_inline_content(&mut state, true);
                }
                TagEnd::Table => {
                    // Keep Table as pulldown_cmark uses TagEnd::Table
                    flush_block_content(&mut state);
                    render_table(&mut state);
                    if !state.block_stack.is_empty() {
                        state.block_stack.pop();
                    }
                    state.ui.add_space(6.0);
                }
                TagEnd::TableHead => {}
                TagEnd::TableRow => {}
                TagEnd::TableCell => {
                    if let Some(last_row) = state.table_rows.last_mut() {
                        last_row.push(state.current_cell_job.clone());
                    }
                    state.current_cell_job = LayoutJob::default();
                    if state.in_table_header {
                        if !state.inline_style_stack.is_empty() {
                            state.inline_style_stack.pop();
                        }
                    }
                }
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    if !state.inline_style_stack.is_empty() {
                        state.inline_style_stack.pop();
                    }
                }
                TagEnd::Link { .. } => {
                    // Keep as struct
                    if let Some(url) = state.current_link_url.take() {
                        let text = std::mem::take(&mut state.current_link_text);
                        if !state.inline_style_stack.is_empty() {
                            state.inline_style_stack.pop();
                        }
                        let response = state.ui.link(text).on_hover_text(&url);
                        if response.clicked() {
                            if let Err(e) = open::that(&url) {
                                log::error!("Failed to open link '{}': {}", url, e);
                            }
                        }
                    } else {
                        if !state.inline_style_stack.is_empty() {
                            state.inline_style_stack.pop();
                        }
                    }
                }
                TagEnd::Image { .. } => {} // Keep as struct
                TagEnd::FootnoteDefinition => {
                    // Keep as struct
                    flush_block_content(&mut state);
                }
                TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {}
            },
            Event::Text(text) => {
                if state.highlighter.is_some() {
                    state.code_block_content.push_str(&text);
                } else if state.current_link_url.is_some() {
                    state.current_link_text.push_str(&text);
                } else {
                    let format = current_format(&state).clone();
                    state.current_job.append(&text, 0.0, format.clone());
                    if state
                        .block_stack
                        .last()
                        .map_or(false, |b| matches!(b, BlockInfo::Table))
                    {
                        state.current_cell_job.append(&text, 0.0, format);
                    }
                }
            }
            Event::Code(text) => {
                let mut code_format = current_format(&state).clone();
                code_format.font_id = FontId::monospace(CODE_FONT_SIZE);
                code_format.background = state.visuals.code_bg_color;
                if state.current_link_url.is_some() {
                    state.current_link_text.push('`');
                    state.current_link_text.push_str(&text);
                    state.current_link_text.push('`');
                } else {
                    state.current_job.append("`", 0.0, code_format.clone());
                    state.current_job.append(&text, 0.0, code_format.clone());
                    state.current_job.append("`", 0.0, code_format.clone());
                    if state
                        .block_stack
                        .last()
                        .map_or(false, |b| matches!(b, BlockInfo::Table))
                    {
                        state
                            .current_cell_job
                            .append(&format!("`{}`", text), 0.0, code_format);
                    }
                }
            }
            Event::Html(html) => {
                if html.trim() == "<br>" || html.trim() == "<br/>" || html.trim() == "<br />" {
                    state
                        .current_job
                        .append("\n", 0.0, current_format(&state).clone());
                } else {
                    log::debug!("Ignoring HTML: {}", html);
                    let mut fmt = current_format(&state).clone();
                    fmt.italics = true;
                    fmt.color = state.visuals.weak_text_color();
                    state
                        .current_job
                        .append(&format!("[HTML: {}]", html.trim()), 0.0, fmt);
                }
            }
            Event::InlineHtml(html) => {
                if html.trim() == "<br>" || html.trim() == "<br/>" || html.trim() == "<br />" {
                    state
                        .current_job
                        .append("\n", 0.0, current_format(&state).clone());
                } else {
                    log::debug!("Ignoring inline HTML: {}", html);
                    let mut fmt = current_format(&state).clone();
                    fmt.italics = true;
                    fmt.color = state.visuals.weak_text_color();
                    state
                        .current_job
                        .append(&format!("[Inline HTML: {}]", html.trim()), 0.0, fmt);
                }
            }
            Event::FootnoteReference(label) => {
                let mut fmt = current_format(&state).clone();
                fmt.font_id.size *= 0.8;
                fmt.valign = egui::Align::TOP;
                let text = format!("[^{}]", label);
                state.current_job.append(&text, 0.0, fmt);
            }
            Event::SoftBreak => {
                if state.highlighter.is_some() {
                    state.code_block_content.push('\n');
                } else if state.current_link_url.is_some() {
                    state.current_link_text.push(' ');
                } else {
                    state
                        .current_job
                        .append(" ", 0.0, current_format(&state).clone());
                }
            }
            Event::HardBreak => {
                if state.highlighter.is_some() {
                    state.code_block_content.push('\n');
                } else if state.current_link_url.is_some() {
                    state.current_link_text.push('\n');
                } else {
                    state
                        .current_job
                        .append("\n", 0.0, current_format(&state).clone());
                }
            }
            Event::Rule => {
                flush_block_content(&mut state);
                state.ui.add_space(8.0);
                state.ui.add(Separator::default().horizontal());
                state.ui.add_space(8.0);
            }
            Event::TaskListMarker(checked) => {
                let marker = if checked { "[x] " } else { "[ ] " };
                let mut task_job = LayoutJob::default();
                task_job.append(marker, 0.0, current_format(&state).clone());
                task_job.append_job(std::mem::take(&mut state.current_job)); // Corrected append
                state.current_job = task_job;
                if state
                    .block_stack
                    .last()
                    .map_or(false, |b| matches!(b, BlockInfo::Table))
                {
                    let mut cell_task_job = LayoutJob::default();
                    cell_task_job.append(marker, 0.0, current_format(&state).clone());
                    cell_task_job.append_job(std::mem::take(&mut state.current_cell_job)); // Corrected append
                    state.current_cell_job = cell_task_job;
                }
            }
        }
    }
    flush_block_content(&mut state);
}

fn current_format<'a>(state: &'a RenderState<'a, '_>) -> &'a TextFormat {
    state
        .inline_style_stack
        .last()
        .unwrap_or(&state.base_format)
}

fn flush_inline_content(state: &mut RenderState<'_, '_>, add_paragraph_spacing: bool) {
    if !state.current_job.is_empty() {
        let job_to_render = std::mem::take(&mut state.current_job);
        state.ui.label(job_to_render);
        if add_paragraph_spacing {
            state.ui.add_space(4.0);
        }
    }
    state.current_job = LayoutJob::default();
}

fn flush_block_content(state: &mut RenderState<'_, '_>) {
    let needs_flush = !state.current_job.is_empty();
    if needs_flush {
        let is_paragraph_like = state.block_stack.is_empty()
            || matches!(
                state.block_stack.last(),
                Some(BlockInfo::List(_)) | Some(BlockInfo::BlockQuote)
            );
        flush_inline_content(state, is_paragraph_like);
    }
}

fn render_code_block(state: &mut RenderState<'_, '_>) {
    let code = std::mem::take(&mut state.code_block_content);
    let _language_name = state.code_language.as_deref().unwrap_or("text");
    let frame = Frame::none()
        .fill(state.visuals.code_bg_color)
        .inner_margin(Margin::symmetric(6.0, 4.0))
        .rounding(Rounding::same(4.0));
    frame.show(state.ui, |ui| {
        ScrollArea::horizontal()
            .id_source(ui.next_auto_id())
            .show(ui, |ui| {
                let mut job = LayoutJob::default();
                if let Some(highlighter) = state.highlighter.as_mut() {
                    for line in LinesWithEndings::from(&code) {
                        match highlighter.highlight_line(line, &SYNTAX_SET) {
                            Ok(ranges) => {
                                for (style, text) in ranges {
                                    job.append(text, 0.0, syntect_style_to_text_format(style));
                                }
                            }
                            Err(e) => {
                                log::error!("Syntect highlighting error: {}", e);
                                job.append(
                                    &code,
                                    0.0,
                                    TextFormat {
                                        font_id: FontId::monospace(CODE_FONT_SIZE),
                                        color: state.visuals.text_color(),
                                        ..Default::default()
                                    },
                                );
                                break;
                            }
                        }
                    }
                } else {
                    job.append(
                        &code,
                        0.0,
                        TextFormat {
                            font_id: FontId::monospace(CODE_FONT_SIZE),
                            color: state.visuals.text_color(),
                            ..Default::default()
                        },
                    );
                }
                ui.add(egui::Label::new(job).wrap(false));
            });
    });
}

fn render_table(state: &mut RenderState<'_, '_>) {
    let rows = std::mem::take(&mut state.table_rows);
    if rows.is_empty() {
        return;
    }
    let num_columns = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if num_columns == 0 {
        return;
    }
    let frame = Frame::none()
        .stroke(Stroke::new(
            1.0,
            state.visuals.widgets.noninteractive.bg_stroke.color,
        ))
        .inner_margin(Margin::same(4.0));
    frame.show(state.ui, |ui| {
        egui::Grid::new(ui.next_auto_id())
            .num_columns(num_columns)
            .striped(true)
            .spacing([10.0, 4.0])
            .show(ui, |ui| {
                for (row_idx, row_data) in rows.iter().enumerate() {
                    for cell_job in row_data {
                        ui.add(egui::Label::new(cell_job.clone()).wrap(true));
                    }
                    ui.end_row();
                    if row_idx == 0 && rows.len() > 1 {
                        ui.separator();
                    }
                }
            });
    });
}

#[allow(deprecated)] // Allow RetainedImage for now
fn render_image(ui: &mut egui::Ui, url: &str, alt_text: &str) {
    let mut cache = IMAGE_CACHE.lock().unwrap();
    let url_string = url.to_string();
    let retained_image = cache.entry(url_string.clone()).or_insert_with(|| {
        log::debug!("Loading image: {}", url);
        // Ensure you have a `placeholder.png` in the `src` directory!
        RetainedImage::from_image_bytes("placeholder", include_bytes!("placeholder.png"))
            .expect("Failed to load placeholder image bytes") // Panic if placeholder fails
    });
    // Use egui::Image now
    let img_widget = Image::new(egui::ImageSource::Texture(egui::load::SizedTexture::new(
        retained_image.texture_id(ui.ctx()),
        retained_image.size_vec2(),
    )))
    .fit_to_original_size(1.0)
    .max_width(ui.available_width() * 0.8);
    ui.add_space(4.0);
    let response = ui.add(img_widget);
    if !alt_text.is_empty() {
        response.on_hover_text(format!("{} ({})", alt_text, url));
    } else {
        response.on_hover_text(url);
    }
    ui.add_space(4.0);
}

fn syntect_style_to_text_format(style: SyntectStyle) -> TextFormat {
    let fg = style.foreground;
    let color = Color32::from_rgba_unmultiplied(fg.r, fg.g, fg.b, fg.a);
    let font_id = FontId::monospace(CODE_FONT_SIZE);
    let is_italic = style
        .font_style
        .contains(syntect::highlighting::FontStyle::ITALIC);
    TextFormat {
        font_id,
        color,
        italics: is_italic,
        ..Default::default()
    }
}

fn heading_font_id(level: HeadingLevel) -> FontId {
    let size = match level {
        HeadingLevel::H1 => 30.0,
        HeadingLevel::H2 => 24.0,
        HeadingLevel::H3 => 20.0,
        HeadingLevel::H4 => 18.0,
        HeadingLevel::H5 => 16.0,
        HeadingLevel::H6 => 14.0,
    };
    FontId::new(size, egui::FontFamily::Proportional)
}

fn heading_spacing(level: HeadingLevel, before: bool) -> f32 {
    match level {
        HeadingLevel::H1 => {
            if before {
                16.0
            } else {
                8.0
            }
        }
        HeadingLevel::H2 => {
            if before {
                12.0
            } else {
                6.0
            }
        }
        HeadingLevel::H3 => {
            if before {
                10.0
            } else {
                5.0
            }
        }
        _ => {
            if before {
                8.0
            } else {
                4.0
            }
        }
    }
}

fn get_parent_list_num(stack: &[BlockInfo]) -> Option<u64> {
    for block in stack.iter().rev().skip(1) {
        if let BlockInfo::List(num_opt) = block {
            return *num_opt;
        }
    }
    None
}

fn handle_dropped_files(ctx: &egui::Context, app_state: &mut MarkdownViewerApp) {
    let dropped_files = ctx.input(|i| i.raw.dropped_files.clone());
    if !dropped_files.is_empty() {
        log::info!("Files dropped: {:?}", dropped_files);
    }
    for file in dropped_files {
        if let Some(ref path) = file.path {
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
                    log::info!("Accepted dropped file: {}", path.display());
                    *app_state = MarkdownViewerApp::new_from_file(path.clone());
                    break;
                } else {
                    log::debug!(
                        "Ignoring dropped file (wrong extension): {}",
                        path.display()
                    );
                }
            } else {
                log::debug!("Ignoring dropped file (no extension): {}", path.display());
            }
        } else {
            log::debug!("Ignoring dropped file (no path component)");
        }
    }
}

#[cfg(windows)]
fn register_default_viewer() -> Result<(), Box<dyn std::error::Error>> {
    let exe_path = env::current_exe()?;
    let exe_path_str = exe_path.to_string_lossy();
    let command_str = format!("\"{}\" \"%1\"", exe_path_str);
    log::info!("Registering command: {}", command_str);
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let (md_key, _disp) = hkcu.create_subkey(r"Software\Classes\.md")?;
    md_key.set_value("", &"MarkdownViewer.Document")?;
    md_key.set_value("Content Type", &"text/markdown")?;
    md_key.set_value("PerceivedType", &"text")?;
    let (prog_id_key, _disp) = hkcu.create_subkey(r"Software\Classes\MarkdownViewer.Document")?;
    prog_id_key.set_value("", &"Markdown Document (Viewer)")?;
    let (command_key, _disp) = prog_id_key.create_subkey(r"shell\open\command")?;
    command_key.set_value("", &command_str)?;
    let (icon_key, _disp) = prog_id_key.create_subkey(r"DefaultIcon")?;
    icon_key.set_value("", &format!("\"{}\",0", exe_path_str))?;
    log::warn!("Windows registry updated. A restart or log-off might be needed for changes to fully apply in Explorer.");
    Ok(())
}

#[cfg(not(windows))]
fn register_default_viewer() -> Result<(), Box<dyn std::error::Error>> {
    Err("Default viewer registration is only supported on Windows.".into())
}

fn current_time() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64())
}

const DEFAULT_MARKDOWN: &str = r#"
# Welcome to Markdown Viewer! 👋
This viewer renders Markdown files using `egui` and `pulldown-cmark`.
## Features
* Basic formatting: **bold**, *italic*, ~~strikethrough~~, `inline code`.
* Headings (like the ones above and below).
* [x] Task lists!
* [ ] More task lists.
* Links like [Rust Lang](https://www.rust-lang.org).
* Ordered lists:
    1.  First item.
    2.  Second item.
        * Nested unordered list.
        * Another nested item.
    3.  Third item.
* Unordered lists:
    * Bullet point.
    -   Another bullet point.
    +   Yet another.
* Blockquotes:
    > This is a quote. It can span multiple lines and contain *other* formatting.
    >
    > > Nested quotes are possible too!
* Code blocks with syntax highlighting:
```rust
// Example Rust code
fn main() {
    let message = "Hello, egui!";
    println!("{}", message);
}
# Example Python code
def greet(name):
  print(f"Hello, {name}!")
greet("World")
Horizontal Rules:

Tables: Header 1 Header 2 Header 3 Cell 1 Center Right Cell 2 code Bold Cell 3 Italic Link

Image display (URL): Rust Logo

Footnotes [^1].

Try it out!

Use 📂 Open or drag & drop a .md file onto the window. "#;

fn main() -> Result<(), eframe::Error> {
    env_logger::init();

    hide_console();

    let args: Vec<String> = env::args().collect();

    let options = NativeOptions {
        viewport: ViewportBuilder::default()
            .with_inner_size([900.0, 700.0])
            .with_drag_and_drop(true),
        persist_window: true,
        ..Default::default()
    };

    let initial_app = if args.len() > 1 {
        let file_path = PathBuf::from(&args[1]);
        if file_path.exists()
            && (file_path
                .extension()
                .map_or(false, |e| e == "md" || e == "markdown"))
        {
            log::info!(
                "Loading initial file from argument: {}",
                file_path.display()
            );
            MarkdownViewerApp::new_from_file(file_path)
        } else {
            log::warn!(
                "Invalid file path or extension provided via argument: {}",
                args[1]
            );
            MarkdownViewerApp::error(format!("Invalid file path provided: {}", args[1]))
        }
    } else {
        MarkdownViewerApp::new_default()
    };

    let app_loaded = |cc: &eframe::CreationContext<'_>| -> Box<dyn App> {
        let mut app = initial_app;

        if let Some(storage) = cc.storage {
            if let Some(dark_mode) = eframe::get_value::<bool>(storage, "dark_mode") {
                app.dark_mode = dark_mode;
                log::info!("Loaded dark_mode state: {}", dark_mode);
            }
        }
        Box::new(app)
    };

    eframe::run_native("Markdown Viewer", options, Box::new(app_loaded))
}

trait LayoutJobExt {
    fn append_job(&mut self, other: LayoutJob);
}

impl LayoutJobExt for LayoutJob {
    fn append_job(&mut self, other: LayoutJob) {
        if other.is_empty() {
            return;
        }
        let start_offset = self.text.len();
        self.text.push_str(&other.text);
        for mut section in other.sections {
            section.byte_range.start += start_offset;
            section.byte_range.end += start_offset;
            self.sections.push(section);
        }
    }
}
