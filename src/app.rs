use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, ensure};
use chrono::NaiveDate;
use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Scrollbar,
        ScrollbarOrientation, ScrollbarState, Tabs, Wrap,
    },
};
use ratatui_textarea::{CursorMove, TextArea};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    config::{Accent, Config, DAILY_TAG},
    store::{Note, Workspace},
};

const LOGO: &str = concat!(
    "┌┬─┐ ┐ ┌┬──┐ ┌─┬┬─┐ ┌┐  ┐\n",
    "├┤ │ │ ├┤  │   ├┤   ├┤  │\n",
    "└┘ └─┘ └┴──┘   └┘   └┴──┘",
);
const LOGO_INITIAL: &str = "┌┬─┐ ┐\n├┤ │ │\n└┘ └─┘";
const LOGO_SMALL: &str = "N";
const HOTKEYS: &str = "^N new  ^D today  ^S save  ^F search  ^T tag  ^G find  ^R reload  ^Q quit  F1 help  F2 mode  F3 view  F4 config  F5 rename  F6 tags  F7 delete  F8 restore  F9 next  F10 prev";

#[derive(Clone, Copy)]
struct MarkdownStyle(Color);

impl tui_markdown::StyleSheet for MarkdownStyle {
    fn heading(&self, level: u8) -> Style {
        let emphasis = match level {
            1 | 2 => Modifier::BOLD,
            3 => Modifier::BOLD | Modifier::ITALIC,
            _ => Modifier::ITALIC,
        };
        Style::default()
            .fg(self.0)
            .bg(Color::Reset)
            .add_modifier(emphasis)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Browse,
    Edit,
    Preview,
    Search,
    Find,
    New,
    Settings,
    Rename,
    Delete,
}

struct DeleteRequest {
    note: Note,
    expected: String,
    return_mode: Mode,
    confirm: bool,
}

enum NoteRow {
    Date(NaiveDate),
    Note(usize),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SettingsPage {
    Appearance,
    Tags,
}

enum TagEditor {
    Name(Box<TextArea<'static>>),
    Color {
        name: String,
        state: ListState,
        new: bool,
    },
}

#[derive(Clone, Copy, Default)]
struct NotePosition {
    cursor: (usize, usize),
    editor_top: usize,
    preview_scroll: u16,
}

pub struct App {
    store: Workspace,
    notes: Vec<Note>,
    filtered: Vec<usize>,
    note_rows: Vec<NoteRow>,
    list_state: ListState,
    current: Option<Note>,
    editor: TextArea<'static>,
    original: String,
    dirty: bool,
    mode: Mode,
    preview: Text<'static>,
    scroll: u16,
    preview_height: usize,
    preview_width: Option<u16>,
    query: String,
    tag_filter: String,
    search_tags: bool,
    search_editor: TextArea<'static>,
    find_editor: TextArea<'static>,
    find_query: String,
    find_return: Mode,
    find_origin: (usize, usize),
    positions: BTreeMap<PathBuf, NotePosition>,
    title_editor: TextArea<'static>,
    delete_request: Option<DeleteRequest>,
    copy: Option<String>,
    status: String,
    help: bool,
    help_scroll: u16,
    help_height: u16,
    help_lines: usize,
    view_height: u16,
    config: Config,
    color_state: ListState,
    settings_return: Mode,
    settings_page: SettingsPage,
    draft_tags: BTreeMap<String, Accent>,
    tag_state: ListState,
    tag_editor: Option<TagEditor>,
    prompt_return: Mode,
    editor_top: usize,
    editor_height: usize,
    restore_editor_top: Option<usize>,
}

impl App {
    pub fn new(store: Workspace, today: bool) -> Result<Self> {
        let (config, config_warning) = Config::load_recovering(&store.root)?;
        let warning = [store.warning.clone(), config_warning]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let mut app = Self {
            store,
            notes: Vec::new(),
            filtered: Vec::new(),
            note_rows: Vec::new(),
            list_state: ListState::default(),
            current: None,
            editor: TextArea::default(),
            original: String::new(),
            dirty: false,
            mode: Mode::Browse,
            preview: Text::default(),
            scroll: 0,
            preview_height: 0,
            preview_width: None,
            query: String::new(),
            tag_filter: String::new(),
            search_tags: false,
            search_editor: title_editor("", config.accent.color()),
            find_editor: title_editor("", config.accent.color()),
            find_query: String::new(),
            find_return: Mode::Browse,
            find_origin: (0, 0),
            positions: BTreeMap::new(),
            title_editor: title_editor("", config.accent.color()),
            delete_request: None,
            copy: None,
            status: String::new(),
            help: false,
            help_scroll: 0,
            help_height: 0,
            help_lines: 0,
            view_height: 0,
            config,
            color_state: ListState::default(),
            settings_return: Mode::Browse,
            settings_page: SettingsPage::Appearance,
            draft_tags: BTreeMap::new(),
            tag_state: ListState::default(),
            tag_editor: None,
            prompt_return: Mode::Browse,
            editor_top: 0,
            editor_height: 0,
            restore_editor_top: None,
        };
        app.reload()?;
        if today {
            let note = app.store.today()?;
            app.reload()?;
            app.open(note, Mode::Edit)?;
        } else if let Some(note) = app.selected() {
            app.open(note, Mode::Browse)?;
        }
        app.status = warning;
        Ok(app)
    }

    fn reload(&mut self) -> Result<()> {
        let selected = self.selected().map(|note| note.path);
        self.notes = self.store.list()?;
        self.filter();
        if let Some(selected) = selected
            && let Some(row) = self.note_rows.iter().position(
                |row| matches!(row, NoteRow::Note(index) if self.notes[*index].path == selected),
            )
        {
            self.list_state.select(Some(row));
        }
        Ok(())
    }

    fn filter(&mut self) {
        let query = self.query.to_lowercase();
        let tag = self.tag_filter.to_lowercase();
        let selected = self.selected().map(|note| note.path);
        self.filtered =
            self.notes
                .iter()
                .enumerate()
                .filter(|(_, note)| {
                    note.title.to_lowercase().contains(&query)
                        && (tag.is_empty()
                            || note.title.split_once(':').is_some_and(|(prefix, _)| {
                                prefix.trim().to_lowercase().contains(&tag)
                            }))
                })
                .map(|(index, _)| index)
                .collect();
        self.note_rows.clear();
        let mut previous = None;
        for &index in &self.filtered {
            let date = self.notes[index].created.date_naive();
            if previous != Some(date) {
                self.note_rows.push(NoteRow::Date(date));
                previous = Some(date);
            }
            self.note_rows.push(NoteRow::Note(index));
        }
        self.list_state.select(self.note_rows.iter().position(|row| matches!(row, NoteRow::Note(index) if Some(&self.notes[*index].path) == selected.as_ref()))
            .or((!self.filtered.is_empty()).then_some(1)));
    }

    fn selected(&self) -> Option<Note> {
        self.list_state
            .selected()
            .and_then(|index| self.note_rows.get(index))
            .and_then(|row| match row {
                NoteRow::Note(index) => self.notes.get(*index).cloned(),
                _ => None,
            })
    }

    fn open(&mut self, note: Note, mode: Mode) -> Result<()> {
        let source = self.store.read(&note)?;
        self.remember_position();
        let position = self.positions.get(&note.path).copied().unwrap_or_default();
        let normalized = source.replace("\r\n", "\n");
        self.editor = TextArea::new(normalized.split('\n').map(str::to_owned).collect());
        self.editor_top = position.editor_top;
        self.restore_editor_top = Some(position.editor_top);
        self.editor.set_style(Style::default().fg(Color::White));
        self.editor
            .set_line_number_style(Style::default().fg(Color::DarkGray));
        self.editor.set_cursor_line_style(Style::default());
        self.editor.set_cursor_style(
            Style::default()
                .fg(Color::Black)
                .bg(self.config.accent.color()),
        );
        self.editor.move_cursor(cursor_jump(position.cursor));
        self.apply_find_pattern()?;
        self.original = source;
        self.dirty = false;
        self.current = Some(note);
        self.mode = mode;
        self.status.clear();
        if mode != Mode::Edit {
            self.render_preview();
        }
        self.scroll = position.preview_scroll;
        if let Some(current) = &self.current {
            let selected = self
                .note_rows
                .iter()
                .position(|row| matches!(row, NoteRow::Note(index) if self.notes[*index].path == current.path));
            self.list_state.select(selected);
        }
        Ok(())
    }

    fn remember_position(&mut self) {
        if let Some(note) = &self.current {
            let cursor = self.editor.cursor();
            self.positions.insert(
                note.path.clone(),
                NotePosition {
                    cursor: (cursor.0, cursor.1),
                    editor_top: self.editor_top,
                    preview_scroll: self.scroll,
                },
            );
        }
    }

    fn clear_filters(&mut self) {
        self.query.clear();
        self.tag_filter.clear();
    }

    fn filter_summary(&self) -> String {
        [
            (!self.tag_filter.is_empty()).then(|| format!("tag: {}", self.tag_filter)),
            (!self.query.is_empty()).then(|| format!("name: {}", self.query)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
    }

    fn content(&self) -> String {
        self.editor.lines().join("\n")
    }

    fn save(&mut self) -> Result<()> {
        if self.dirty
            && let Some(note) = &self.current
        {
            let content = self.content();
            self.store.save(note, &self.original, &content)?;
            self.original = content;
            self.dirty = false;
            self.status = "Saved".into();
        }
        Ok(())
    }

    fn render_preview(&mut self) {
        // Parsed once on open / mode change, never on every cursor movement.
        let content = self.content();
        let options = tui_markdown::Options::new(MarkdownStyle(self.config.accent.color()));
        let rendered = tui_markdown::from_str_with_options(&content, &options);
        self.preview = Text {
            style: rendered.style,
            alignment: rendered.alignment,
            lines: rendered
                .lines
                .into_iter()
                .map(|line| Line {
                    style: line.style,
                    alignment: line.alignment,
                    spans: line
                        .spans
                        .into_iter()
                        .map(|span| Span::styled(span.content.into_owned(), span.style))
                        .collect(),
                })
                .collect(),
        };
        self.preview_width = None;
        self.scroll = 0;
    }

    fn today(&mut self) -> Result<()> {
        self.save()?;
        let note = self.store.today()?;
        self.clear_filters();
        self.reload()?;
        self.open(note, Mode::Edit)?;
        self.status = "Today's note".into();
        Ok(())
    }

    fn start_new(&mut self) {
        self.copy = None;
        if let Err(error) = self.save() {
            self.copy = Some(self.content());
            self.status = format!("Error: {error:#} · Enter a title for the copy");
        }
        self.title_editor = title_editor("", self.config.accent.color());
        self.mode = Mode::New;
    }

    fn create(&mut self) -> Result<()> {
        let note = self.store.create(&self.title_editor.lines()[0])?;
        self.clear_filters();
        self.reload()?;
        self.open(note, Mode::Edit)?;
        if let Some(copy) = self.copy.take() {
            self.editor = TextArea::new(copy.split('\n').map(str::to_owned).collect());
            self.dirty = true;
            self.save()?;
        }
        self.status = "Note created".into();
        Ok(())
    }

    fn browse(&mut self) -> Result<()> {
        let saved = self.dirty;
        self.save()?;
        self.clear_filters();
        self.filter();
        self.list_state
            .select(self.current.as_ref().and_then(|note| {
                self.note_rows.iter().position(
                |row| matches!(row, NoteRow::Note(index) if self.notes[*index].path == note.path),
            )
            }));
        self.mode = Mode::Browse;
        self.render_preview();
        self.status = if saved { "Saved automatically" } else { "" }.into();
        Ok(())
    }

    fn settings(&mut self) -> Result<()> {
        if self.mode == Mode::Delete {
            self.cancel_delete();
        }
        self.save()?;
        self.settings_return = self.mode;
        self.settings_page = SettingsPage::Appearance;
        self.draft_tags.clone_from(&self.config.tags);
        self.tag_state.select(Some(0));
        self.tag_editor = None;
        self.color_state.select(
            Accent::ALL
                .iter()
                .position(|&accent| accent == self.config.accent),
        );
        self.mode = Mode::Settings;
        self.status.clear();
        Ok(())
    }

    fn selected_accent(&self) -> Accent {
        self.color_state
            .selected()
            .and_then(|index| Accent::ALL.get(index))
            .copied()
            .unwrap_or(self.config.accent)
    }

    fn toggle_preview(&mut self) {
        if self.current.is_none() {
            return;
        }
        if self.mode == Mode::Preview {
            self.edit();
        } else {
            self.render_preview();
            self.mode = Mode::Preview;
            self.status.clear();
        }
    }

    fn edit(&mut self) {
        self.mode = Mode::Edit;
        self.status.clear();
    }

    fn move_selection(&mut self, delta: isize) -> Result<()> {
        if self.filtered.is_empty() {
            return Ok(());
        }
        let current = self.list_state.selected().unwrap_or(1);
        let next = match delta {
            isize::MIN => self
                .note_rows
                .iter()
                .position(|row| matches!(row, NoteRow::Note(_))),
            isize::MAX => self
                .note_rows
                .iter()
                .rposition(|row| matches!(row, NoteRow::Note(_))),
            n if n < 0 => (0..current)
                .rev()
                .find(|&index| matches!(self.note_rows[index], NoteRow::Note(_))),
            _ => (current + 1..self.note_rows.len())
                .find(|&index| matches!(self.note_rows[index], NoteRow::Note(_))),
        }
        .unwrap_or(current);
        let NoteRow::Note(index) = self.note_rows[next] else {
            return Ok(());
        };
        let note = self.notes[index].clone();
        let mode = if self.mode == Mode::Search {
            Mode::Search
        } else {
            Mode::Browse
        };
        if self
            .current
            .as_ref()
            .is_some_and(|current| current.path == note.path)
        {
            self.list_state.select(Some(next));
            return Ok(());
        }
        self.open(note, mode)
    }

    fn search(&mut self) -> Result<()> {
        self.save()?;
        self.filter();
        self.search_editor = title_editor(
            if self.search_tags {
                &self.tag_filter
            } else {
                &self.query
            },
            self.config.accent.color(),
        );
        self.search_editor.select_all();
        self.mode = Mode::Search;
        self.status.clear();
        Ok(())
    }

    fn update_search(&mut self) -> Result<()> {
        let value = self.search_editor.lines()[0].to_owned();
        if self.search_tags {
            self.tag_filter = value;
        } else {
            self.query = value;
        }
        self.filter();
        if let Some(note) = self.selected() {
            if self
                .current
                .as_ref()
                .is_none_or(|current| current.path != note.path)
            {
                self.open(note, Mode::Search)?;
            }
        } else {
            self.remember_position();
            self.current = None;
            self.preview = Text::default();
        }
        Ok(())
    }

    fn apply_find_pattern(&mut self) -> Result<()> {
        // Searches are literal and Unicode case-insensitive; regex syntax is escaped.
        let escaped = self
            .find_query
            .chars()
            .map(|c| {
                if "\\.^$|?*+()[]{}".contains(c) {
                    format!("\\{c}")
                } else {
                    c.to_string()
                }
            })
            .collect::<String>();
        self.editor.set_search_pattern(if escaped.is_empty() {
            String::new()
        } else {
            format!("(?i){escaped}")
        })?;
        self.editor.set_search_style(
            Style::default()
                .fg(self.config.accent.color())
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD),
        );
        Ok(())
    }

    fn start_find(&mut self) {
        if self.mode == Mode::Delete {
            self.cancel_delete();
        }
        if self.current.is_none() {
            self.status = "No note selected".into();
            return;
        }
        self.find_return = if matches!(self.mode, Mode::Browse | Mode::Edit | Mode::Preview) {
            self.mode
        } else {
            Mode::Edit
        };
        let cursor = self.editor.cursor();
        self.find_origin = (cursor.0, cursor.1);
        self.find_editor = title_editor(&self.find_query, self.config.accent.color());
        self.find_editor.select_all();
        self.mode = Mode::Find;
        self.status.clear();
    }

    fn update_find(&mut self) -> Result<()> {
        self.find_query = self.find_editor.lines()[0].to_owned();
        self.apply_find_pattern()?;
        self.editor.move_cursor(cursor_jump(self.find_origin));
        let found = self.editor.search_forward(true);
        self.find_status(found);
        Ok(())
    }

    fn find_status(&mut self, found: bool) {
        let count = self
            .editor
            .search_pattern()
            .map(|pattern| {
                self.editor
                    .lines()
                    .iter()
                    .map(|line| pattern.find_iter(line).count())
                    .sum::<usize>()
            })
            .unwrap_or(0);
        self.status = if self.find_query.is_empty() {
            "".into()
        } else if found {
            format!("{count} matches · {}", self.find_query)
        } else {
            format!("No matches · {}", self.find_query)
        };
    }

    fn next_match(&mut self, backwards: bool) -> Result<()> {
        if self.find_query.is_empty() || self.current.is_none() {
            self.start_find();
            return Ok(());
        }
        self.apply_find_pattern()?;
        self.editor.cancel_selection();
        let found = if backwards {
            self.editor.search_back(false)
        } else {
            self.editor.search_forward(false)
        };
        if self.mode != Mode::Find {
            self.mode = Mode::Edit;
        }
        self.find_status(found);
        Ok(())
    }

    fn start_rename(&mut self) -> Result<()> {
        self.save()?;
        if matches!(self.mode, Mode::Browse | Mode::Search) {
            let Some(note) = self.selected() else {
                self.status = "No note selected".into();
                return Ok(());
            };
            if self
                .current
                .as_ref()
                .is_none_or(|current| current.path != note.path)
            {
                self.open(note, Mode::Browse)?;
            }
        }
        if let Some(note) = &self.current {
            self.title_editor = title_editor(&note.title, self.config.accent.color());
            self.prompt_return = if matches!(self.mode, Mode::Edit | Mode::Preview | Mode::Browse) {
                self.mode
            } else {
                Mode::Browse
            };
            self.mode = Mode::Rename;
            self.status.clear();
        }
        Ok(())
    }

    fn rename(&mut self) -> Result<()> {
        if let Some(note) = self.current.clone() {
            self.remember_position();
            let renamed = self.store.rename(&note, &self.title_editor.lines()[0])?;
            if let Some(position) = self.positions.get(&note.path).copied() {
                self.positions.insert(renamed.path.clone(), position);
            }
            self.clear_filters();
            self.reload()?;
            self.open(renamed, self.prompt_return)?;
            self.status = "Note renamed".into();
        }
        Ok(())
    }

    fn start_delete(&mut self) -> Result<()> {
        let target = if matches!(self.mode, Mode::Browse | Mode::Search) {
            self.selected()
        } else {
            self.current.clone()
        };
        let Some(note) = target else {
            self.status = "No note selected".into();
            return Ok(());
        };
        self.save()?;
        let return_mode = self.mode;
        if self
            .current
            .as_ref()
            .is_none_or(|current| current.path != note.path)
        {
            self.open(note.clone(), Mode::Browse)?;
        }
        self.delete_request = Some(DeleteRequest {
            note,
            expected: self.original.clone(),
            return_mode,
            confirm: false,
        });
        self.render_preview();
        self.mode = Mode::Delete;
        self.status.clear();
        Ok(())
    }

    fn cancel_delete(&mut self) {
        if let Some(request) = self.delete_request.take() {
            self.mode = request.return_mode;
        } else if self.mode == Mode::Delete {
            self.mode = Mode::Browse;
        }
        self.status.clear();
    }

    fn delete(&mut self) -> Result<()> {
        let Some(request) = &self.delete_request else {
            return Ok(());
        };
        self.store.delete(&request.note, &request.expected)?;
        let next = self
            .notes
            .iter()
            .position(|note| note.path == request.note.path)
            .unwrap_or(0);
        self.delete_request = None;
        self.current = None;
        self.editor = TextArea::default();
        self.original.clear();
        self.dirty = false;
        self.preview = Text::default();
        self.editor_top = 0;
        self.copy = None;
        self.mode = Mode::Browse;
        self.clear_filters();
        self.reload()?;
        if let Some(note) = self
            .notes
            .get(next.min(self.notes.len().saturating_sub(1)))
            .cloned()
        {
            self.open(note, Mode::Browse)?;
        }
        self.status = "Note deleted · F8 restores it".into();
        Ok(())
    }

    fn restore(&mut self) -> Result<()> {
        self.save()?;
        let note = self.store.restore()?;
        self.clear_filters();
        self.reload()?;
        self.open(note, Mode::Browse)?;
        self.status = "Note restored".into();
        Ok(())
    }

    fn active_prompt(&mut self) -> Option<&mut TextArea<'static>> {
        match self.mode {
            Mode::New | Mode::Rename => Some(&mut self.title_editor),
            Mode::Search => Some(&mut self.search_editor),
            Mode::Find => Some(&mut self.find_editor),
            Mode::Settings => match &mut self.tag_editor {
                Some(TagEditor::Name(input)) => Some(input),
                _ => None,
            },
            _ => None,
        }
    }

    fn prompt_changed(&mut self) -> Result<()> {
        match self.mode {
            Mode::Search => self.update_search(),
            Mode::Find => self.update_find(),
            _ => Ok(()),
        }
    }

    fn save_settings(&mut self) -> Result<()> {
        while self.tag_editor.is_some() {
            self.confirm_tag_editor()?;
        }
        let config = Config {
            accent: self.selected_accent(),
            tags: self.draft_tags.clone(),
        };
        config.save(&self.store.root)?;
        self.config = config;
        if self.current.is_some() {
            let scroll = self.scroll;
            self.render_preview();
            self.scroll = scroll;
        }
        self.mode = self.settings_return;
        self.status = "Settings saved".into();
        Ok(())
    }

    fn selected_tag(&self) -> Option<String> {
        self.draft_tags
            .keys()
            .nth(self.tag_state.selected().unwrap_or(0))
            .cloned()
    }

    fn edit_tag_color(&mut self, name: String) {
        let color = self.draft_tags[&name];
        self.tag_editor = Some(TagEditor::Color {
            name,
            state: ListState::default().with_selected(Accent::ALL.iter().position(|&c| c == color)),
            new: false,
        });
    }

    fn confirm_tag_editor(&mut self) -> Result<()> {
        match &self.tag_editor {
            Some(TagEditor::Name(input)) => {
                let name = input.lines()[0].trim().to_owned();
                Config::validate_tag(&name)?;
                ensure!(
                    !self.draft_tags.contains_key(&name),
                    "This tag already exists"
                );
                self.tag_editor = Some(TagEditor::Color {
                    name,
                    state: ListState::default().with_selected(self.color_state.selected()),
                    new: true,
                });
            }
            Some(TagEditor::Color { name, state, .. }) => {
                let color = Accent::ALL[state.selected().unwrap_or(0)];
                self.draft_tags.insert(name.clone(), color);
                self.tag_state
                    .select(self.draft_tags.keys().position(|tag| tag == name));
                self.tag_editor = None;
            }
            None => {}
        }
        self.status.clear();
        Ok(())
    }

    fn settings_key(&mut self, key: KeyEvent) -> Result<()> {
        if matches!(self.tag_editor, Some(TagEditor::Name(_))) {
            match key.code {
                KeyCode::Esc => {
                    self.tag_editor = None;
                    self.status.clear();
                }
                KeyCode::Enter => self.confirm_tag_editor()?,
                _ => {
                    if let Some(TagEditor::Name(input)) = &mut self.tag_editor {
                        single_line_input(input, key);
                    }
                }
            }
            return Ok(());
        }
        if self.tag_editor.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.tag_editor = None;
                    self.status.clear();
                }
                KeyCode::Enter => self.confirm_tag_editor()?,
                KeyCode::Up | KeyCode::Down => {
                    if let Some(TagEditor::Color { state, .. }) = &mut self.tag_editor {
                        let index = state.selected().unwrap_or(0);
                        state.select(Some(if key.code == KeyCode::Up {
                            index.saturating_sub(1)
                        } else {
                            (index + 1).min(Accent::ALL.len() - 1)
                        }));
                    }
                }
                _ => {}
            }
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.mode = self.settings_return;
                self.status = "Settings unchanged".into();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                self.settings_page = if self.settings_page == SettingsPage::Appearance {
                    SettingsPage::Tags
                } else {
                    SettingsPage::Appearance
                };
                self.status.clear();
            }
            KeyCode::Enter => {
                if self.settings_page == SettingsPage::Appearance {
                    self.save_settings()?;
                } else if let Some(name) = self.selected_tag() {
                    self.edit_tag_color(name);
                } else {
                    self.tag_editor = Some(TagEditor::Name(Box::new(title_editor(
                        "",
                        self.config.accent.color(),
                    ))));
                }
            }
            KeyCode::Up | KeyCode::Char('k') | KeyCode::Down | KeyCode::Char('j') => {
                let (state, max) = if self.settings_page == SettingsPage::Appearance {
                    (&mut self.color_state, Accent::ALL.len() - 1)
                } else {
                    (&mut self.tag_state, self.draft_tags.len())
                };
                let index = state.selected().unwrap_or(0);
                let up = matches!(key.code, KeyCode::Up | KeyCode::Char('k'));
                state.select(Some(if up {
                    index.saturating_sub(1)
                } else {
                    (index + 1).min(max)
                }));
            }
            KeyCode::Delete if self.settings_page == SettingsPage::Tags => {
                if let Some(name) = self.selected_tag() {
                    if name == DAILY_TAG {
                        self.status = "daily is built in; only its color can be changed".into();
                        return Ok(());
                    }
                    self.draft_tags.remove(&name);
                    let index = self
                        .tag_state
                        .selected()
                        .unwrap_or(0)
                        .min(self.draft_tags.len());
                    self.tag_state.select(Some(index));
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn note_title<'a>(&self, title: &'a str) -> Line<'a> {
        if let Some((tag, _)) = title.split_once(':') {
            let tag_name = tag.trim();
            let tags = if self.mode == Mode::Settings {
                &self.draft_tags
            } else {
                &self.config.tags
            };
            let preview = match &self.tag_editor {
                Some(TagEditor::Color { name, state, .. })
                    if self.mode == Mode::Settings && name == tag_name =>
                {
                    Some(Accent::ALL[state.selected().unwrap_or(0)])
                }
                _ => None,
            };
            if let Some(color) = preview.or_else(|| tags.get(tag_name).copied()) {
                return Line::from(vec![
                    Span::styled(tag, Style::default().fg(color.color())),
                    Span::raw(&title[tag.len()..]),
                ]);
            }
        }
        Line::from(title)
    }

    fn draw_settings(&mut self, frame: &mut Frame, body: Rect, accent: Color) {
        let block = Block::bordered()
            .title(" Settings ")
            .border_style(Style::default().fg(accent));
        let inner = block.inner(body);
        frame.render_widget(block, body);
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(if inner.height >= 10 { 2 } else { 0 }),
        ])
        .split(inner);
        frame.render_widget(
            Tabs::new(vec!["Primary color", "Tags"])
                .select(if self.settings_page == SettingsPage::Appearance {
                    0
                } else {
                    1
                })
                .style(Style::default().fg(Color::DarkGray))
                .highlight_style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
            parts[0],
        );
        if let Some(TagEditor::Color { name, state, new }) = &mut self.tag_editor {
            let color = Accent::ALL[state.selected().unwrap_or(0)];
            let rows = Layout::vertical([
                Constraint::Length(u16::from(parts[1].height >= 3)),
                Constraint::Min(1),
            ])
            .split(parts[1]);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::raw("Color: "),
                    Span::styled(name.as_str(), Style::default().fg(color.color())),
                ])),
                rows[0],
            );
            let items = color_items(
                self.draft_tags
                    .get(name)
                    .copied()
                    .unwrap_or(Accent::ALL[state.selected().unwrap_or(0)]),
                if *new { "new" } else { "current" },
            );
            clamp_list_offset(state, Accent::ALL.len(), rows[1].height as usize);
            frame.render_stateful_widget(
                List::new(items).highlight_symbol("> ").highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
                rows[1],
                state,
            );
        } else if self.settings_page == SettingsPage::Appearance {
            clamp_list_offset(
                &mut self.color_state,
                Accent::ALL.len(),
                parts[1].height as usize,
            );
            frame.render_stateful_widget(
                List::new(color_items(self.config.accent, "saved"))
                    .highlight_symbol("> ")
                    .highlight_style(
                        Style::default()
                            .fg(Color::Black)
                            .bg(accent)
                            .add_modifier(Modifier::BOLD),
                    ),
                parts[1],
                &mut self.color_state,
            );
        } else {
            let mut items: Vec<_> = self
                .draft_tags
                .iter()
                .map(|(name, color)| {
                    ListItem::new(Line::from(vec![
                        Span::styled(name.as_str(), Style::default().fg(color.color())),
                        Span::styled(
                            format!("  ● {}", color.label()),
                            Style::default().fg(Color::DarkGray),
                        ),
                        Span::styled(
                            if name == DAILY_TAG { " (built-in)" } else { "" },
                            Style::default().fg(Color::DarkGray),
                        ),
                    ]))
                })
                .collect();
            items.push(ListItem::new("+ New tag").style(Style::default().fg(accent)));
            clamp_list_offset(
                &mut self.tag_state,
                self.draft_tags.len() + 1,
                parts[1].height as usize,
            );
            frame.render_stateful_widget(
                List::new(items).highlight_symbol("> ").highlight_style(
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                ),
                parts[1],
                &mut self.tag_state,
            );
            draw_scrollbar(
                frame,
                parts[1],
                self.draft_tags.len() + 1,
                parts[1].height as usize,
                self.tag_state.offset(),
                accent,
            );
        }
        let description = if self.settings_page == SettingsPage::Appearance {
            "Applies to active panels, selection and cursor.\nSaved for this workspace."
        } else {
            "A prefix before ':' is a tag.\nColors are saved for this workspace."
        };
        frame.render_widget(
            Paragraph::new(description)
                .wrap(Wrap { trim: false })
                .style(Style::default().fg(Color::DarkGray)),
            parts[2],
        );
        if let Some(TagEditor::Name(input)) = &mut self.tag_editor {
            let popup = centered(body, 64, 5);
            frame.render_widget(Clear, popup);
            input.set_cursor_style(Style::default().fg(Color::Black).bg(accent));
            input.set_block(
                Block::bordered()
                    .title(" New tag ")
                    .border_style(Style::default().fg(accent)),
            );
            frame.render_widget(input.as_ref(), popup);
        }
    }

    pub fn event(&mut self, event: Event) -> bool {
        let result = (|| -> Result<bool> {
            match event {
                Event::Key(key) if key.kind != KeyEventKind::Release => self.key(key),
                Event::Paste(text) => {
                    match self.mode {
                        Mode::Edit => {
                            self.dirty |= self.editor.insert_str(text);
                        }
                        Mode::Search => {
                            self.search_editor.insert_str(
                                text.chars().filter(|c| !c.is_control()).collect::<String>(),
                            );
                            self.update_search()?;
                        }
                        Mode::Find => {
                            self.find_editor.insert_str(
                                text.chars().filter(|c| !c.is_control()).collect::<String>(),
                            );
                            self.update_find()?;
                        }
                        Mode::New | Mode::Rename => {
                            self.title_editor.insert_str(
                                text.chars().filter(|c| !c.is_control()).collect::<String>(),
                            );
                        }
                        Mode::Settings => {
                            if let Some(TagEditor::Name(input)) = &mut self.tag_editor {
                                input.insert_str(
                                    text.chars().filter(|c| !c.is_control()).collect::<String>(),
                                );
                            }
                        }
                        _ => {}
                    }
                    Ok(false)
                }
                _ => Ok(false),
            }
        })();
        if self.mode != Mode::Delete {
            self.delete_request = None;
        }
        match result {
            Ok(quit) => quit,
            Err(error) => {
                self.status = format!("Error: {error:#}");
                false
            }
        }
    }

    fn key(&mut self, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::F(1) {
            self.help = !self.help;
            self.help_scroll = 0;
            return Ok(false);
        }
        if self.help {
            match key.code {
                KeyCode::Down | KeyCode::Char('j') => {
                    self.help_scroll = self.help_scroll.saturating_add(1);
                    return Ok(false);
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    self.help_scroll = self.help_scroll.saturating_sub(1);
                    return Ok(false);
                }
                KeyCode::PageDown => {
                    self.help_scroll = self.help_scroll.saturating_add(self.help_height);
                    return Ok(false);
                }
                KeyCode::PageUp => {
                    self.help_scroll = self.help_scroll.saturating_sub(self.help_height);
                    return Ok(false);
                }
                KeyCode::Home => {
                    self.help_scroll = 0;
                    return Ok(false);
                }
                KeyCode::End => {
                    self.help_scroll = u16::MAX;
                    return Ok(false);
                }
                _ => {}
            }
            self.help = false;
            if !matches!(key.code, KeyCode::F(2..=10))
                && !key.modifiers.contains(KeyModifiers::CONTROL)
            {
                return Ok(false);
            }
        }
        if self.mode == Mode::Delete
            && (matches!(key.code, KeyCode::F(2..=6 | 8..=10))
                || (key.modifiers.contains(KeyModifiers::CONTROL)
                    && matches!(
                        key.code,
                        KeyCode::Char('n' | 'd' | 'f' | 't' | 'g' | 'r' | 'p')
                    )))
        {
            self.cancel_delete();
        }
        if key.code == KeyCode::F(8) {
            self.restore()?;
            return Ok(false);
        }
        if key.code == KeyCode::F(9) || key.code == KeyCode::F(10) {
            self.next_match(key.code == KeyCode::F(10))?;
            return Ok(false);
        }
        if key.code == KeyCode::F(2) {
            if self.mode == Mode::Browse {
                if self.current.is_some() {
                    self.edit();
                }
            } else {
                self.browse()?;
                self.copy = None;
            }
            return Ok(false);
        }
        if key.code == KeyCode::F(3) {
            if !matches!(self.mode, Mode::Browse | Mode::Edit | Mode::Preview) {
                self.browse()?;
                self.copy = None;
            }
            self.toggle_preview();
            return Ok(false);
        }
        if key.code == KeyCode::F(5) {
            self.start_rename()?;
            return Ok(false);
        }
        if key.code == KeyCode::F(4) {
            if self.mode == Mode::Settings {
                self.mode = self.settings_return;
                self.status = "Settings unchanged".into();
            } else {
                self.settings()?;
            }
            return Ok(false);
        }
        if key.code == KeyCode::F(6) {
            if self.mode != Mode::Settings {
                self.settings()?;
            }
            self.settings_page = SettingsPage::Tags;
            self.tag_editor = None;
            self.status.clear();
            return Ok(false);
        }
        if key.code == KeyCode::F(7) {
            if self.mode == Mode::Delete {
                self.cancel_delete();
            } else {
                self.start_delete()?;
            }
            return Ok(false);
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if let Some(input) = self.active_prompt() {
                        input.copy();
                    } else {
                        self.editor.copy();
                    }
                    return Ok(false);
                }
                KeyCode::Char('v' | 'z' | 'y') => {
                    let changed = if let Some(input) = self.active_prompt() {
                        match key.code {
                            KeyCode::Char('v') => input.paste(),
                            KeyCode::Char('z') => input.undo(),
                            _ => input.redo(),
                        }
                    } else if self.mode == Mode::Edit {
                        let changed = match key.code {
                            KeyCode::Char('v') => self.editor.paste(),
                            KeyCode::Char('z') => self.editor.undo(),
                            _ => self.editor.redo(),
                        };
                        self.dirty |= changed;
                        false
                    } else {
                        false
                    };
                    if changed {
                        self.prompt_changed()?;
                    }
                    return Ok(false);
                }
                KeyCode::Char('q') => {
                    self.save()?;
                    return Ok(true);
                }
                KeyCode::Char('s') => {
                    match self.mode {
                        Mode::Settings => self.save_settings()?,
                        Mode::New => self.create()?,
                        Mode::Rename => self.rename()?,
                        Mode::Find => self.mode = Mode::Edit,
                        _ => self.save()?,
                    }
                    return Ok(false);
                }
                KeyCode::Char('d') => {
                    self.today()?;
                    return Ok(false);
                }
                KeyCode::Char('n') => {
                    self.start_new();
                    return Ok(false);
                }
                KeyCode::Char('f') => {
                    self.search_tags = false;
                    self.search()?;
                    return Ok(false);
                }
                KeyCode::Char('t') => {
                    self.search_tags = true;
                    self.search()?;
                    return Ok(false);
                }
                KeyCode::Char('g') => {
                    self.start_find();
                    return Ok(false);
                }
                KeyCode::Char('r') => {
                    self.save()?;
                    let mode = match self.mode {
                        Mode::Edit | Mode::Preview => self.mode,
                        _ => Mode::Browse,
                    };
                    let path = self.current.as_ref().map(|note| note.path.clone());
                    self.reload()?;
                    let note = self
                        .notes
                        .iter()
                        .find(|note| Some(&note.path) == path.as_ref())
                        .cloned()
                        .or_else(|| self.selected());
                    if let Some(note) = note {
                        self.open(note, mode)?;
                    } else {
                        self.current = None;
                        self.preview = Text::default();
                        self.mode = Mode::Browse;
                    }
                    self.status = "Workspace refreshed".into();
                    return Ok(false);
                }
                KeyCode::Char('p') => {
                    if !matches!(self.mode, Mode::Browse | Mode::Edit | Mode::Preview) {
                        self.browse()?;
                    }
                    self.toggle_preview();
                    return Ok(false);
                }
                _ => {}
            }
        }
        match self.mode {
            Mode::Delete => match key.code {
                KeyCode::Esc => self.cancel_delete(),
                KeyCode::Char('n' | 'N')
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.cancel_delete()
                }
                KeyCode::Char('y' | 'Y')
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.delete()?
                }
                KeyCode::Enter => {
                    if self
                        .delete_request
                        .as_ref()
                        .is_some_and(|request| request.confirm)
                    {
                        self.delete()?;
                    } else {
                        self.cancel_delete();
                    }
                }
                KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                    if let Some(request) = &mut self.delete_request {
                        request.confirm = match key.code {
                            KeyCode::Left => false,
                            KeyCode::Right => true,
                            _ => !request.confirm,
                        };
                    }
                }
                _ => {}
            },
            Mode::Rename => match key.code {
                KeyCode::Esc => self.mode = self.prompt_return,
                KeyCode::Enter => self.rename()?,
                _ => {
                    single_line_input(&mut self.title_editor, key);
                }
            },
            Mode::Settings => self.settings_key(key)?,
            Mode::New => match key.code {
                KeyCode::Esc => {
                    self.copy = None;
                    self.mode = if self.current.is_some() {
                        Mode::Edit
                    } else {
                        Mode::Browse
                    };
                }
                KeyCode::Enter => self.create()?,
                _ => {
                    single_line_input(&mut self.title_editor, key);
                }
            },
            Mode::Search => match key.code {
                KeyCode::Esc => {
                    self.clear_filters();
                    self.filter();
                    if let Some(note) = self.selected() {
                        self.open(note, Mode::Browse)?;
                    }
                    self.mode = Mode::Browse;
                }
                KeyCode::Enter => {
                    if let Some(note) = self.selected() {
                        self.open(note, Mode::Browse)?;
                    } else {
                        self.current = None;
                        self.preview = Text::default();
                    }
                    self.mode = Mode::Browse;
                }
                KeyCode::Up => self.move_selection(-1)?,
                KeyCode::Down => self.move_selection(1)?,
                _ => {
                    let before = self.search_editor.lines()[0].to_owned();
                    single_line_input(&mut self.search_editor, key);
                    if self.search_editor.lines()[0] != before {
                        self.update_search()?;
                    }
                }
            },
            Mode::Find => match key.code {
                KeyCode::Esc => {
                    self.find_query.clear();
                    self.apply_find_pattern()?;
                    self.editor.move_cursor(cursor_jump(self.find_origin));
                    self.mode = self.find_return;
                    self.status.clear();
                }
                KeyCode::Enter => self.mode = Mode::Edit,
                _ => {
                    let before = self.find_editor.lines()[0].to_owned();
                    single_line_input(&mut self.find_editor, key);
                    if self.find_editor.lines()[0] != before {
                        self.update_find()?;
                    }
                }
            },
            Mode::Edit => match key.code {
                KeyCode::Esc | KeyCode::F(2) => self.browse()?,
                _ => {
                    match key.code {
                        KeyCode::PageDown => {
                            self.editor_top = self
                                .editor_top
                                .saturating_add(self.editor_height)
                                .min(u16::MAX as usize)
                        }
                        KeyCode::PageUp => {
                            self.editor_top = self.editor_top.saturating_sub(self.editor_height)
                        }
                        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::ALT) => {
                            self.editor_top = self.editor_top.saturating_sub(self.editor_height)
                        }
                        _ => {}
                    }
                    self.dirty |= self.editor.input(key);
                }
            },
            Mode::Preview => match key.code {
                KeyCode::Esc | KeyCode::F(2) => self.browse()?,
                KeyCode::Enter => self.edit(),
                KeyCode::Down | KeyCode::Char('j') => self.scroll = self.scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => self.scroll = self.scroll.saturating_sub(1),
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(self.view_height),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(self.view_height),
                KeyCode::Home => self.scroll = 0,
                KeyCode::End => self.scroll = u16::MAX,
                _ => {}
            },
            Mode::Browse => match key.code {
                KeyCode::Enter => {
                    if self.current.is_some() {
                        self.edit();
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => self.move_selection(1)?,
                KeyCode::Up | KeyCode::Char('k') => self.move_selection(-1)?,
                KeyCode::Home => self.move_selection(isize::MIN)?,
                KeyCode::End => self.move_selection(isize::MAX)?,
                KeyCode::Esc => {
                    self.clear_filters();
                    self.filter();
                    if let Some(note) = self.selected() {
                        self.open(note, Mode::Browse)?;
                    }
                }
                _ => {}
            },
        }
        Ok(false)
    }

    fn footer(&self, width: u16, accent: Color) -> Paragraph<'static> {
        let mode = if self.help {
            "HELP"
        } else {
            match self.mode {
                Mode::Browse => "LIST",
                Mode::Edit => "EDIT",
                Mode::Preview => "PREVIEW",
                Mode::Search => "SEARCH",
                Mode::Find => "FIND",
                Mode::New => "NEW",
                Mode::Settings => "SETTINGS",
                Mode::Rename => "RENAME",
                Mode::Delete => "DELETE",
            }
        };
        let dim = Style::default().fg(Color::DarkGray);
        let items = std::iter::once(Span::styled(
            format!("{mode:<8}"),
            Style::default().fg(accent).add_modifier(Modifier::BOLD),
        ))
        .chain(HOTKEYS.split("  ").map(|hint| Span::styled(hint, dim)))
        .chain(std::iter::once(Span::styled(
            concat!("v", env!("CARGO_PKG_VERSION")),
            dim,
        )));
        let mut lines = Vec::new();
        let mut line = Line::default();
        for item in items {
            // Keep a shortcut and its label together whenever the width permits.
            if !line.spans.is_empty() && line.width() + 2 + item.width() > width as usize {
                lines.push(std::mem::take(&mut line));
            }
            if !line.spans.is_empty() {
                line.spans.push(Span::raw("  "));
            }
            line.spans.push(item);
        }
        lines.push(line);
        Paragraph::new(Text::from(lines)).wrap(Wrap { trim: true })
    }

    pub fn draw(&mut self, frame: &mut Frame) {
        let area = frame.area();
        let accent = if self.mode == Mode::Settings {
            self.selected_accent().color()
        } else {
            self.config.accent.color()
        };
        let controls_width = area.width.saturating_sub(2);
        let footer = self.footer(controls_width, accent);
        let footer_height = footer
            .line_count(controls_width)
            .min(area.height.saturating_sub(1) as usize) as u16;
        let status_color = if self.status.starts_with("Error:") {
            Color::LightRed
        } else if self.status.starts_with("Warning:") {
            Color::Yellow
        } else {
            Color::DarkGray
        };
        let status = Paragraph::new(shorten_line(
            Line::from(self.status.as_str()),
            controls_width as usize,
        ))
        .style(Style::default().fg(status_color));
        // Reserve one row regardless of message length: save notifications never move the editor.
        let status_height = u16::from(area.height > footer_height + 1);
        let rows = Layout::vertical([
            Constraint::Min(1),
            Constraint::Length(status_height),
            Constraint::Length(footer_height),
        ])
        .split(area);
        frame.render_widget(status, rows[1].inner(Margin::new(1, 0)));
        frame.render_widget(footer, rows[2].inner(Margin::new(1, 0)));
        if area.width < 40 || rows[0].height < 4 {
            let logo = if area.width >= 10 && rows[0].height >= 5 {
                LOGO_INITIAL
            } else {
                LOGO_SMALL
            };
            let padding = Padding::new(
                if area.width >= 10 { 2 } else { 0 },
                if area.width >= 10 { 2 } else { 0 },
                if rows[0].height >= 3 { 1 } else { 0 },
                0,
            );
            frame.render_widget(
                Paragraph::new(format!("{logo}\n\nnotu · enlarge terminal to show notes"))
                    .block(Block::default().padding(padding))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(accent)),
                rows[0],
            );
            return;
        }
        // Keep both panes visible and make the sidebar follow terminal width.
        let sidebar_width = (area.width as u32 * 30 / 100).max(12) as u16;
        let columns = Layout::horizontal([Constraint::Length(sidebar_width), Constraint::Min(1)])
            .split(rows[0]);
        let (sidebar, body) = (columns[0], columns[1]);
        {
            let list_active = matches!(self.mode, Mode::Browse | Mode::Search);
            let logo_height = if sidebar.height >= 8 {
                5
            } else if sidebar.height >= 6 {
                3
            } else {
                1
            };
            let logo = if logo_height < 5 {
                LOGO_SMALL
            } else if sidebar.width >= 29 {
                LOGO
            } else {
                LOGO_INITIAL
            };
            let summary = self.filter_summary();
            let parts = Layout::vertical([
                Constraint::Length(logo_height),
                Constraint::Length(u16::from(!summary.is_empty())),
                Constraint::Min(1),
            ])
            .split(sidebar);
            frame.render_widget(
                Paragraph::new(logo)
                    .block(Block::default().padding(Padding::new(
                        2,
                        2,
                        u16::from(logo_height > 1),
                        u16::from(logo_height > 1),
                    )))
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(accent).add_modifier(Modifier::BOLD)),
                parts[0],
            );
            frame.render_widget(
                Paragraph::new(shorten_line(
                    Line::from(summary.as_str()),
                    sidebar.width.saturating_sub(2) as usize,
                ))
                .style(Style::default().fg(Color::DarkGray)),
                parts[1].inner(Margin::new(1, 0)),
            );
            let row_width = parts[2].width.saturating_sub(4) as usize;
            let selected_index = self
                .list_state
                .selected()
                .and_then(|row| self.note_rows.get(row))
                .and_then(|row| match row {
                    NoteRow::Note(index) => Some(*index),
                    _ => None,
                });
            let items: Vec<_> = self
                .note_rows
                .iter()
                .map(|row| match row {
                    NoteRow::Note(index) => ListItem::new(shorten_line(
                        self.note_title(&self.notes[*index].title),
                        row_width,
                    ))
                    .style(Style::default().fg(
                        if list_active && selected_index == Some(*index) {
                            accent
                        } else {
                            Color::Gray
                        },
                    )),
                    NoteRow::Date(date) => {
                        let label = if row_width >= 15 {
                            let prefix = format!("--- {} ", date.format("%d.%m.%Y"));
                            format!(
                                "{prefix}{}",
                                "-".repeat(row_width.saturating_sub(prefix.len()))
                            )
                        } else if row_width >= 10 {
                            date.format("%d.%m.%Y").to_string()
                        } else {
                            date.format("%d.%m").to_string()
                        };
                        ListItem::new(label).style(Style::default().fg(Color::DarkGray))
                    }
                })
                .collect();
            let title = format!(" {} ", self.filtered.len());
            let block =
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(if list_active {
                        accent
                    } else {
                        Color::DarkGray
                    }));
            let highlight = if list_active {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().bg(Color::DarkGray)
            };
            let mut list_state = self.list_state;
            clamp_list_offset(
                &mut list_state,
                self.note_rows.len(),
                parts[2].height.saturating_sub(2) as usize,
            );
            frame.render_stateful_widget(
                List::new(items)
                    .style(Style::default().fg(Color::Gray))
                    .block(block)
                    .highlight_style(highlight)
                    .highlight_symbol("> "),
                parts[2],
                &mut list_state,
            );
            self.list_state = list_state;
            draw_scrollbar(
                frame,
                parts[2].inner(Margin::new(0, 1)),
                self.note_rows.len(),
                parts[2].height.saturating_sub(2) as usize,
                self.list_state.offset(),
                if list_active { accent } else { Color::Gray },
            );
        }
        if self.mode == Mode::Settings {
            self.draw_settings(frame, body, accent);
        } else if let Some(note) = &self.current {
            let mut title = Line::from(
                shorten_line(
                    self.note_title(&note.title),
                    body.width.saturating_sub(if self.dirty { 6 } else { 4 }) as usize,
                )
                .spans
                .into_iter()
                .map(|span| Span::styled(span.content.into_owned(), span.style))
                .collect::<Vec<_>>(),
            );
            title.spans.insert(0, Span::raw(" "));
            title
                .spans
                .push(Span::raw(if self.dirty { " * " } else { " " }));
            let block = Block::bordered()
                .title(title)
                .border_style(Style::default().fg(
                    if matches!(self.mode, Mode::Edit | Mode::Preview | Mode::Find) {
                        accent
                    } else {
                        Color::DarkGray
                    },
                ));
            if matches!(self.mode, Mode::Edit | Mode::Find) {
                self.editor
                    .set_cursor_style(Style::default().fg(Color::Black).bg(accent));
                self.editor.set_block(block);
                self.editor_height = body.height.saturating_sub(2) as usize;
                frame.render_widget(&self.editor, body);
                if let Some(saved_top) = self.restore_editor_top.take() {
                    let cursor = self.editor.cursor();
                    let minimum = (cursor.0 + 1).saturating_sub(self.editor_height);
                    let top = saved_top.max(minimum).min(cursor.0);
                    // A fresh textarea starts at zero, then reveals its cursor on first render.
                    let mut delta = top as isize - minimum as isize;
                    while delta != 0 {
                        let step = delta.clamp(i16::MIN as isize, i16::MAX as isize) as i16;
                        self.editor.scroll((step, 0));
                        delta -= step as isize;
                    }
                    self.editor.move_cursor(cursor_jump((cursor.0, cursor.1)));
                    self.editor_top = top;
                    frame.render_widget(&self.editor, body);
                }
                let cursor = self.editor.cursor().0;
                if cursor < self.editor_top {
                    self.editor_top = cursor;
                } else if cursor >= self.editor_top + self.editor_height {
                    self.editor_top = (cursor + 1).saturating_sub(self.editor_height);
                }
                draw_scrollbar(
                    frame,
                    body.inner(Margin::new(0, 1)),
                    self.editor.lines().len(),
                    self.editor_height,
                    self.editor_top,
                    accent,
                );
            } else {
                self.view_height = body.height.saturating_sub(2);
                let width = body.width.saturating_sub(2);
                if self.preview_width != Some(width) {
                    self.preview_height = Paragraph::new(borrow_text(&self.preview))
                        .wrap(Wrap { trim: false })
                        .line_count(width);
                    self.preview_width = Some(width);
                }
                let max_scroll = self
                    .preview_height
                    .saturating_sub(body.height.saturating_sub(2) as usize)
                    .min(u16::MAX as usize) as u16;
                self.scroll = self.scroll.min(max_scroll);
                frame.render_widget(
                    Paragraph::new(borrow_text(&self.preview))
                        .block(block)
                        .wrap(Wrap { trim: false })
                        .scroll((self.scroll, 0)),
                    body,
                );
                draw_scrollbar(
                    frame,
                    body.inner(Margin::new(0, 1)),
                    self.preview_height,
                    self.view_height as usize,
                    self.scroll as usize,
                    if self.mode == Mode::Preview {
                        accent
                    } else {
                        Color::Gray
                    },
                );
            }
        } else {
            frame.render_widget(
                Paragraph::new("\n  Your Markdown workspace\n\n  No note is open.")
                    .block(Block::bordered().title(" Welcome "))
                    .style(Style::default().fg(Color::DarkGray)),
                body,
            );
        }
        if matches!(
            self.mode,
            Mode::New | Mode::Search | Mode::Rename | Mode::Find
        ) {
            let popup = centered(body, 64, 5);
            frame.render_widget(Clear, popup);
            let (input, title) = match self.mode {
                Mode::Search => (
                    &mut self.search_editor,
                    if self.search_tags {
                        " Search tags "
                    } else {
                        " Search names "
                    },
                ),
                Mode::Find => (&mut self.find_editor, " Find in note "),
                Mode::New => (&mut self.title_editor, " New note title "),
                _ => (&mut self.title_editor, " Rename note "),
            };
            input.set_cursor_style(Style::default().fg(Color::Black).bg(accent));
            input.set_block(
                Block::bordered()
                    .title(title)
                    .border_style(Style::default().fg(accent)),
            );
            frame.render_widget(&*input, popup);
        }
        if self.mode == Mode::Delete
            && let Some(request) = &self.delete_request
        {
            let filename = request
                .note
                .path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&request.note.title);
            let name = Paragraph::new(self.note_title(filename)).wrap(Wrap { trim: false });
            let width = body.width.saturating_sub(2).min(64);
            let height =
                (name.line_count(width.saturating_sub(2)) + 3).min(u16::MAX as usize) as u16;
            let popup = centered(body, 64, height.max(4));
            frame.render_widget(Clear, popup);
            let block = Block::bordered()
                .title(" Delete note? ")
                .border_style(Style::default().fg(Color::Red));
            let inner = block.inner(popup);
            frame.render_widget(block, popup);
            let rows = Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).split(inner);
            frame.render_widget(name, rows[0]);
            let active = Style::default()
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD);
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        "[ Cancel ]",
                        if !request.confirm {
                            active.fg(accent)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                    Span::raw("   "),
                    Span::styled(
                        "[ Delete ]",
                        if request.confirm {
                            active.fg(Color::Red)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        },
                    ),
                ])),
                rows[1],
            );
        }
        if self.help {
            let popup = centered(body, 76, 28);
            frame.render_widget(Clear, popup);
            let help = Text::from(vec![
                Line::from("NOTES   ↑/↓ or j/k: select · Enter: edit"),
                Line::from(""),
                Line::from("GLOBAL  Ctrl+D: today · Ctrl+N: new · Ctrl+S: save"),
                Line::from("        Ctrl+F: search · Ctrl+R: reload · Ctrl+Q: quit"),
                Line::from("        F1: help · F2: list/edit · F3: preview"),
                Line::from("        F4: settings · F5: rename filename"),
                Line::from("        F6: tags · F7: delete · F8: restore deleted note"),
                Line::from("        Ctrl+T: tag filter · Ctrl+G: find in current note"),
                Line::from("        F9: next match · F10: previous match (wraps)"),
                Line::from("        ^ in the footer means Ctrl"),
                Line::from("PROMPTS Enter / Ctrl+S: confirm · Esc: cancel"),
                Line::from("DELETE  Y: delete · N / Esc: cancel · ←/→ + Enter: choose"),
                Line::from("SEARCH  ↑/↓: results · ←/→, Home/End: edit query"),
                Line::from("        Name and tag filters combine; Esc clears them"),
                Line::from("TAGS    ↑/↓: select · Enter: add / edit tag color"),
                Line::from("        Delete: remove · Ctrl+S: apply · Esc: cancel"),
                Line::from("EDITOR  Type Markdown · arrows / Home / End: move"),
                Line::from("        Ctrl+Z: undo · Ctrl+Y: redo · Shift+arrows: select"),
                Line::from("        Ctrl+X/C/V: internal cut / copy / paste"),
                Line::from("        Terminal paste is supported (bracketed paste)"),
                Line::from("        Esc / F2: save, return to list · F3: preview"),
                Line::from(""),
                Line::from("PREVIEW ↑/↓, PageUp/Down, Home/End: scroll · Enter: edit"),
                Line::from("Files save on Ctrl+S, switching notes and normal exit."),
                Line::from("A failed save keeps edits open. Ctrl+N can save a copy."),
                Line::from("HELP    ↑/↓, PageUp/Down, Home/End: scroll · Esc: close"),
                Line::from(""),
                Line::from(format!("STATUS  {}", self.status)),
            ]);
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" notu / keys ")
                .border_style(Style::default().fg(accent));
            let inner = block.inner(popup);
            let paragraph = Paragraph::new(help).wrap(Wrap { trim: false });
            self.help_lines = paragraph.line_count(inner.width);
            self.help_height = inner.height;
            let max = self
                .help_lines
                .saturating_sub(inner.height as usize)
                .min(u16::MAX as usize) as u16;
            self.help_scroll = self.help_scroll.min(max);
            frame.render_widget(paragraph.block(block).scroll((self.help_scroll, 0)), popup);
            draw_scrollbar(
                frame,
                popup.inner(Margin::new(0, 1)),
                self.help_lines,
                inner.height as usize,
                self.help_scroll as usize,
                accent,
            );
        }
    }
}

fn title_editor(value: &str, accent: Color) -> TextArea<'static> {
    let mut editor = TextArea::new(vec![value.to_owned()]);
    editor.set_style(Style::default().fg(Color::White));
    editor.set_cursor_line_style(Style::default());
    editor.set_cursor_style(Style::default().fg(Color::Black).bg(accent));
    editor.move_cursor(CursorMove::End);
    editor
}

fn cursor_jump(cursor: (usize, usize)) -> CursorMove {
    CursorMove::Jump(
        cursor.0.min(u16::MAX as usize) as u16,
        cursor.1.min(u16::MAX as usize) as u16,
    )
}

fn shorten_line(line: Line<'_>, width: usize) -> Line<'_> {
    if line.width() <= width {
        return line;
    }
    let mut shortened = Line {
        style: line.style,
        alignment: line.alignment,
        ..Line::default()
    };
    if width == 0 {
        return shortened;
    }
    let mut remaining = width - 1;
    'spans: for span in line.spans {
        let mut content = String::new();
        for grapheme in span.content.graphemes(true) {
            let cells = Span::raw(grapheme).width();
            if cells > remaining {
                shortened.spans.push(Span::styled(content, span.style));
                break 'spans;
            }
            content.push_str(grapheme);
            remaining -= cells;
        }
        shortened.spans.push(Span::styled(content, span.style));
    }
    shortened.spans.push(Span::raw("…"));
    shortened
}

fn single_line_input(editor: &mut TextArea<'_>, key: KeyEvent) {
    if key.code == KeyCode::Enter
        || matches!(key.code, KeyCode::Char(c) if c.is_control())
        || (key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('j' | 'm')))
    {
        return;
    }
    editor.input(key);
}

fn clamp_list_offset(state: &mut ListState, total: usize, visible: usize) {
    let offset = state.offset().min(total.saturating_sub(visible));
    *state.offset_mut() = offset;
}

fn color_items(current: Accent, label: &str) -> Vec<ListItem<'static>> {
    Accent::ALL
        .iter()
        .map(|&choice| {
            ListItem::new(format!(
                "● {}{}",
                choice.label(),
                if choice == current {
                    format!(" ({label})")
                } else {
                    String::new()
                }
            ))
            .style(Style::default().fg(choice.color()))
        })
        .collect()
}

fn centered(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width.saturating_sub(2));
    let height = height.min(area.height);
    Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    )
}

fn draw_scrollbar(
    frame: &mut Frame,
    track: Rect,
    total: usize,
    visible: usize,
    top: usize,
    color: Color,
) {
    if total <= visible || visible == 0 {
        return;
    }
    let max_scroll = total - visible;
    let mut state = ScrollbarState::new(max_scroll + 1)
        .position(top.min(max_scroll))
        .viewport_content_length(visible);
    frame.render_stateful_widget(
        Scrollbar::new(ScrollbarOrientation::VerticalRight)
            .begin_symbol(None)
            .end_symbol(None)
            .track_symbol(Some("│"))
            .thumb_symbol("┃")
            .track_style(Style::default().fg(Color::DarkGray))
            .thumb_style(Style::default().fg(color)),
        track,
        &mut state,
    );
}

// Borrow cached span contents instead of copying the note's strings on each frame.
fn borrow_text<'a>(text: &'a Text<'_>) -> Text<'a> {
    Text {
        style: text.style,
        alignment: text.alignment,
        lines: text
            .lines
            .iter()
            .map(|line| Line {
                style: line.style,
                alignment: line.alignment,
                spans: line
                    .spans
                    .iter()
                    .map(|span| Span::styled(span.content.as_ref(), span.style))
                    .collect(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{Terminal, backend::TestBackend};

    fn key(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn ctrl(code: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(code), KeyModifiers::CONTROL))
    }

    fn screen(app: &mut App, width: u16, height: u16) -> Result<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height))?;
        terminal.draw(|frame| app.draw(frame))?;
        Ok(terminal
            .backend()
            .buffer()
            .content
            .chunks(width as usize)
            .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    #[test]
    fn daily_edit_preview_save_and_reopen() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let mut app = App::new(store, false)?;
        assert!(!app.event(ctrl('d')));
        assert_eq!(app.mode, Mode::Edit);
        app.event(Event::Paste("**Привет**\n- [x] Готово\n".into()));
        app.event(key(KeyCode::F(3)));
        assert!(app.preview.to_string().contains("Привет"));
        let mut terminal = Terminal::new(TestBackend::new(100, 30))?;
        terminal.draw(|frame| app.draw(frame))?;
        app.event(key(KeyCode::Esc));
        assert!(!app.dirty);
        assert!(
            app.store
                .read(app.current.as_ref().unwrap())?
                .contains("**Привет**")
        );
        assert!(app.event(ctrl('q')));
        let reopened = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        assert!(reopened.content().contains("**Привет**"));
        Ok(())
    }

    #[test]
    fn save_conflict_blocks_exit_and_can_create_copy() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        app.event(Event::Paste("my edits".into()));
        let path = app.current.as_ref().unwrap().path.clone();
        std::fs::write(&path, "external")?;
        assert!(!app.event(Event::Key(KeyEvent::new(
            KeyCode::Char('q'),
            KeyModifiers::CONTROL
        ))));
        assert!(app.dirty);
        app.event(Event::Key(KeyEvent::new(
            KeyCode::Char('n'),
            KeyModifiers::CONTROL,
        )));
        app.event(Event::Paste("Recovery".into()));
        app.event(key(KeyCode::Enter));
        assert_eq!(std::fs::read_to_string(path)?, "external");
        assert!(std::fs::read_to_string(dir.path().join("Recovery.md"))?.contains("my edits"));
        Ok(())
    }

    #[test]
    fn search_with_no_matches_does_not_edit_a_hidden_note() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        store.create("Идеи")?;
        let mut app = App::new(store, false)?;
        app.event(ctrl('f'));
        app.event(Event::Paste("missing".into()));
        app.event(key(KeyCode::Enter));
        app.event(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Browse);
        assert!(app.current.is_none());
        app.event(key(KeyCode::Esc));
        assert_eq!(app.current.as_ref().unwrap().title, "Идеи");
        Ok(())
    }

    #[test]
    fn editor_undo_redo_and_copy_do_not_exit() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        let before = app.content();
        app.event(Event::Paste("Привет 🦀".into()));
        let after = app.content();
        for (code, expected) in [('z', &before), ('y', &after), ('c', &after)] {
            assert!(!app.event(Event::Key(KeyEvent::new(
                KeyCode::Char(code),
                KeyModifiers::CONTROL
            ))));
            assert_eq!(&app.content(), expected);
        }
        Ok(())
    }

    #[test]
    fn global_new_and_rename_keep_title_separate_from_body() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?;
        for mode in [
            Mode::Browse,
            Mode::Edit,
            Mode::Preview,
            Mode::Search,
            Mode::New,
            Mode::Settings,
            Mode::Rename,
        ] {
            app.mode = mode;
            app.event(ctrl('n'));
            assert_eq!(app.mode, Mode::New);
            app.event(Event::Paste(format!("{mode:?}")));
            app.event(key(KeyCode::Enter));
            assert_eq!(app.mode, Mode::Edit);
            assert_eq!(app.content(), "");
            assert_eq!(app.store.read(app.current.as_ref().unwrap())?, "");
        }
        app.event(Event::Paste("# Body heading\n\nUnchanged content".into()));
        let body = app.content();
        let original = app.current.clone().unwrap();
        app.event(key(KeyCode::F(5)));
        assert_eq!(app.mode, Mode::Rename);
        assert_eq!(app.store.read(&original)?, body);
        for _ in original.title.chars() {
            app.event(key(KeyCode::Backspace));
        }
        app.event(Event::Paste("Переименовано".into()));
        app.event(ctrl('s'));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.current.as_ref().unwrap().title, "Переименовано");
        assert!(!original.path.exists());
        assert_eq!(app.content(), body);
        app.event(key(KeyCode::F(2)));
        assert_eq!(app.selected().unwrap().title, "Переименовано");
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rename_edits_unicode_in_the_middle_and_keeps_the_body() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Привет 🦀")?;
        store.save(&note, "", "Body stays unchanged")?;
        let mut app = App::new(store, false)?;
        app.event(key(KeyCode::F(5)));
        app.event(key(KeyCode::Home));
        app.event(Event::Paste("Тег: ".into()));
        for _ in 0..6 {
            app.event(Event::Key(KeyEvent::new(
                KeyCode::Right,
                KeyModifiers::SHIFT,
            )));
        }
        app.event(Event::Paste("Заметка".into()));
        app.event(key(KeyCode::End));
        app.event(key(KeyCode::Left));
        app.event(key(KeyCode::Delete));
        app.event(Event::Paste("文".into()));
        app.event(key(KeyCode::Backspace));
        app.event(Event::Paste("🦀".into()));
        let expected = "Тег: Заметка 🦀";
        assert_eq!(app.title_editor.lines(), &[expected]);
        app.event(ctrl('z'));
        assert_eq!(app.title_editor.lines(), &["Тег: Заметка "]);
        app.event(ctrl('y'));
        app.event(ctrl('j')); // A title cannot acquire a second line.
        assert_eq!(app.title_editor.lines(), &[expected]);
        app.event(key(KeyCode::Enter));
        assert_eq!(app.current.as_ref().unwrap().title, expected);
        assert_eq!(app.content(), "Body stays unchanged");
        assert_eq!(
            app.store.read(app.current.as_ref().unwrap())?,
            "Body stays unchanged"
        );
        assert!(!note.path.exists());
        // A long title scrolls horizontally to keep the real cursor visible.
        app.event(key(KeyCode::F(5)));
        app.event(key(KeyCode::Home));
        app.event(Event::Paste("x".repeat(100)));
        app.event(key(KeyCode::End));
        assert!(screen(&mut app, 40, 12)?.contains("🦀"));
        app.event(key(KeyCode::Esc));
        assert_eq!(app.current.as_ref().unwrap().title, expected);
        Ok(())
    }

    #[test]
    fn delete_requires_confirmation_and_handles_the_last_note() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.create("First")?;
        let second = store.create("Second")?;
        let mut app = App::new(store, false)?;
        assert_eq!(app.current.as_ref().unwrap().path, second.path);
        for cancel in [
            KeyCode::Enter,
            KeyCode::Esc,
            KeyCode::Char('n'),
            KeyCode::F(7),
        ] {
            app.event(key(KeyCode::F(7)));
            assert_eq!(app.mode, Mode::Delete);
            assert!(!app.delete_request.as_ref().unwrap().confirm);
            assert!(second.path.exists());
            for (width, height) in [(40, 12), (100, 28)] {
                let text = screen(&mut app, width, height)?;
                assert!(text.contains("Delete note?"));
                assert!(text.contains("Second.md"));
                assert!(text.contains("[ Cancel ]   [ Delete ]"));
                assert!(text.contains("F7 delete"));
            }
            app.event(ctrl('s'));
            app.event(ctrl('y'));
            assert!(second.path.exists());
            app.event(key(cancel));
            assert_eq!(app.mode, Mode::Browse);
            assert!(app.delete_request.is_none());
            assert!(second.path.exists());
        }
        app.event(key(KeyCode::F(7)));
        app.event(key(KeyCode::Right));
        app.event(key(KeyCode::Enter));
        assert!(!second.path.exists());
        assert_eq!(app.current.as_ref().unwrap().path, first.path);
        app.event(key(KeyCode::F(7)));
        app.event(key(KeyCode::Char('y')));
        assert!(!first.path.exists());
        assert!(app.current.is_none());
        assert!(app.store.list()?.is_empty());
        assert_eq!(app.mode, Mode::Browse);
        assert!(screen(&mut app, 100, 28)?.contains("No note is open"));
        app.event(ctrl('n'));
        app.event(Event::Paste("After deletion".into()));
        app.event(key(KeyCode::Enter));
        assert!(app.content().is_empty());
        Ok(())
    }

    #[test]
    fn delete_from_search_targets_the_visible_selection_and_refuses_external_changes() -> Result<()>
    {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.create("First")?;
        let other = store.create("Second")?;
        let mut app = App::new(store, false)?;
        app.event(ctrl('f'));
        app.event(Event::Paste("missing".into()));
        app.event(key(KeyCode::F(7)));
        assert_eq!(app.mode, Mode::Search);
        assert!(app.delete_request.is_none());
        assert!(first.path.exists() && other.path.exists());
        app.event(ctrl('f'));
        app.event(Event::Paste("First".into()));
        app.event(key(KeyCode::F(7)));
        assert_eq!(app.delete_request.as_ref().unwrap().note.path, first.path);
        std::fs::write(&first.path, "External edit")?;
        app.event(key(KeyCode::Char('y')));
        assert_eq!(app.mode, Mode::Delete);
        assert!(app.status.contains("changed outside"));
        assert_eq!(std::fs::read_to_string(&first.path)?, "External edit");
        app.event(key(KeyCode::Esc));
        app.event(ctrl('r'));
        app.event(key(KeyCode::F(7)));
        app.event(key(KeyCode::Char('y')));
        assert!(!first.path.exists());
        assert!(other.path.exists());
        assert_eq!(app.current.as_ref().unwrap().path, other.path);
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("Unsaved changes".into()));
        std::fs::write(&other.path, "New external edit")?;
        app.event(key(KeyCode::F(7)));
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.delete_request.is_none());
        assert!(app.dirty);
        assert!(app.content().contains("Unsaved changes"));
        Ok(())
    }

    #[test]
    fn date_groups_and_scrollbars_follow_navigation_and_overflow() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let dates = [("2026-09-30", 15), ("2026-09-26", 15)];
        let mut timestamps = std::collections::BTreeMap::new();
        for (date, count) in dates {
            let timestamp = NaiveDate::parse_from_str(date, "%Y-%m-%d")?
                .and_hms_opt(12, 0, 0)
                .unwrap()
                .and_local_timezone(chrono::Local)
                .unwrap()
                .timestamp_millis();
            for i in 0..count {
                let note = store.create(&format!("{date}-{i:02}"))?;
                timestamps.insert(
                    note.path.file_name().unwrap().to_str().unwrap().to_owned(),
                    timestamp,
                );
            }
        }
        std::fs::write(
            dir.path().join(".notu-created.json"),
            serde_json::to_vec(&timestamps)?,
        )?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?;
        let mut terminal = Terminal::new(TestBackend::new(100, 20))?;
        terminal.draw(|frame| app.draw(frame))?;
        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(29, 6)].symbol(), "┃");
        let header: String = (2..29).map(|x| buffer[(x, 6)].symbol()).collect();
        assert!(header.contains("--- 30.09.2026"));
        assert_eq!(buffer[(2, 6)].fg, Color::DarkGray);
        for _ in 0..15 {
            app.event(key(KeyCode::Down));
        }
        assert_eq!(
            app.selected().unwrap().created.date_naive(),
            NaiveDate::parse_from_str("2026-09-26", "%Y-%m-%d")?
        );
        app.event(key(KeyCode::End));
        terminal.draw(|frame| app.draw(frame))?;
        assert_eq!(terminal.backend().buffer()[(29, 15)].symbol(), "┃");
        app.event(key(KeyCode::Enter));
        terminal.draw(|frame| app.draw(frame))?;
        assert_eq!(terminal.backend().buffer()[(99, 15)].symbol(), "│");
        let text = (0..60)
            .map(|i| format!("line-{i:02}"))
            .collect::<Vec<_>>()
            .join("\n");
        app.event(Event::Paste(text));
        terminal.draw(|frame| app.draw(frame))?;
        assert_eq!(terminal.backend().buffer()[(99, 15)].symbol(), "┃");
        app.editor.move_cursor(CursorMove::Top);
        for page in [None, Some(KeyCode::PageDown), Some(KeyCode::PageUp)] {
            if let Some(code) = page {
                app.event(key(code));
            }
            terminal.draw(|frame| app.draw(frame))?;
            let top: String = (31..99)
                .map(|x| terminal.backend().buffer()[(x, 1)].symbol())
                .collect();
            assert!(
                top.contains(&format!("line-{:02}", app.editor_top)),
                "viewport disagrees with scrollbar: {top}"
            );
        }
        assert_eq!(terminal.backend().buffer()[(99, 1)].symbol(), "┃");
        let mut large = Terminal::new(TestBackend::new(100, 100))?;
        large.draw(|frame| app.draw(frame))?;
        assert!(
            large
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| cell.symbol() != "┃")
        );
        Ok(())
    }

    #[test]
    fn returning_to_full_list_autosaves_and_keeps_selection() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        store.create("Ideas")?;
        let other = store.create("Work")?;
        let mut app = App::new(store, false)?;
        for return_key in [KeyCode::Esc, KeyCode::F(2)] {
            app.event(ctrl('f'));
            app.event(Event::Paste("Ideas".into()));
            app.event(key(KeyCode::Enter));
            app.event(key(KeyCode::Enter));
            app.event(Event::Paste("New text\n".into()));
            let expected = app.content();
            app.event(key(return_key));
            assert_eq!(app.mode, Mode::Browse);
            assert!(!app.dirty);
            assert_eq!(app.filtered.len(), 2);
            assert!(app.query.is_empty());
            assert_eq!(app.selected().unwrap().title, "Ideas");
            assert_eq!(app.store.read(app.current.as_ref().unwrap())?, expected);
        }
        assert_eq!(app.store.read(&other)?, "");
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("Unsaved edit".into()));
        std::fs::write(&app.current.as_ref().unwrap().path, "external")?;
        app.event(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Edit);
        assert!(app.dirty);
        assert!(app.content().contains("Unsaved edit"));
        Ok(())
    }

    #[test]
    fn settings_preview_cancel_and_save_persist_accent() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        app.event(Event::Paste("# Auto saved before settings".into()));
        app.event(key(KeyCode::F(4)));
        assert_eq!(app.mode, Mode::Settings);
        assert!(!app.dirty);
        assert!(
            app.store
                .read(app.current.as_ref().unwrap())?
                .contains("Auto saved before settings")
        );
        app.event(key(KeyCode::Down));
        assert_eq!(app.selected_accent(), Accent::Blue);
        assert_eq!(app.config.accent, Accent::Cyan);
        let mut terminal = Terminal::new(TestBackend::new(100, 28))?;
        terminal.draw(|frame| app.draw(frame))?;
        assert_eq!(
            terminal.backend().buffer()[(30, 0)].fg,
            Accent::Blue.color()
        );
        assert!(
            terminal.backend().buffer().content[..100]
                .iter()
                .all(|cell| cell.bg == Color::Reset)
        );
        app.event(key(KeyCode::Esc));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(Config::load(dir.path())?.accent, Accent::Cyan);
        app.event(key(KeyCode::F(4)));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(app.config.accent, Accent::Green);
        assert_eq!(app.store.list()?.len(), 1);
        let reopened = App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?;
        assert_eq!(reopened.config.accent, Accent::Green);
        app.browse()?;
        app.event(key(KeyCode::F(4)));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        assert_eq!(app.mode, Mode::Browse);
        terminal.draw(|frame| app.draw(frame))?;
        let heading = &terminal.backend().buffer()[(31, 1)];
        assert_eq!(heading.symbol(), "#");
        assert_eq!(heading.fg, Accent::Magenta.color());
        assert_eq!(heading.bg, Color::Reset);
        assert!(heading.modifier.contains(Modifier::BOLD));
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn tags_create_edit_preview_cancel_and_persist_in_settings() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("asd: name")?;
        assert_eq!(note.path.file_name().unwrap(), "asd: name.md");
        store.save(&note, "", "# Separate body\n")?;
        let mut app = App::new(store, false)?;
        app.event(key(KeyCode::F(4)));
        app.event(key(KeyCode::Tab));
        assert!(app.settings_page == SettingsPage::Tags);
        app.event(key(KeyCode::Down)); // The built-in daily tag comes before New tag.
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste(" asd ".into()));
        app.event(key(KeyCode::Enter));
        assert!(matches!(app.tag_editor, Some(TagEditor::Color { .. })));
        for _ in 0..4 {
            app.event(key(KeyCode::Down));
        }
        let narrow = screen(&mut app, 40, 12)?;
        assert!(narrow.contains("> ● Yellow"));
        assert!(narrow.contains("F6 tags"));
        assert_eq!(
            app.note_title("asd: name").spans[0].style.fg,
            Some(Accent::Yellow.color())
        );
        assert!(!Config::load(dir.path())?.tags.contains_key("asd"));
        app.event(key(KeyCode::Enter));
        app.event(ctrl('s'));
        assert_eq!(app.mode, Mode::Browse);
        assert_eq!(Config::load(dir.path())?.tags["asd"], Accent::Yellow);
        let mut terminal = Terminal::new(TestBackend::new(100, 28))?;
        // Even the selected note keeps the tag's own foreground color.
        for mode in [Mode::Browse, Mode::Edit, Mode::Preview] {
            app.mode = mode;
            terminal.draw(|frame| app.draw(frame))?;
            let buffer = terminal.backend().buffer();
            for x in 3..6 {
                assert_eq!(buffer[(x, 7)].fg, Accent::Yellow.color());
            }
            for x in 32..35 {
                assert_eq!(buffer[(x, 0)].fg, Accent::Yellow.color());
            }
            assert_ne!(buffer[(7, 7)].fg, Accent::Yellow.color());
            assert_ne!(buffer[(36, 0)].fg, Accent::Yellow.color());
        }
        assert_eq!(
            app.note_title(" asd : name").spans[0].style.fg,
            Some(Accent::Yellow.color())
        );
        assert_eq!(app.note_title("unknown: name").spans[0].style.fg, None);
        assert_eq!(app.note_title("ASD: name").spans[0].style.fg, None);
        assert_eq!(app.store.read(&note)?, "# Separate body\n");
        app.event(key(KeyCode::F(6)));
        assert!(app.settings_page == SettingsPage::Tags);
        app.event(key(KeyCode::Enter));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        assert_eq!(app.draft_tags["asd"], Accent::Red);
        app.event(key(KeyCode::Esc));
        assert_eq!(Config::load(dir.path())?.tags["asd"], Accent::Yellow);
        app.event(key(KeyCode::F(6)));
        app.event(key(KeyCode::Delete));
        assert_eq!(app.draft_tags, Config::default().tags);
        app.event(key(KeyCode::Esc));
        assert_eq!(app.config.tags["asd"], Accent::Yellow);
        let reopened = App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?;
        assert_eq!(
            reopened.note_title("asd: name").spans[0].style.fg,
            Some(Accent::Yellow.color())
        );
        Ok(())
    }

    #[test]
    fn invalid_and_duplicate_tag_input_can_be_corrected_or_cancelled() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?;
        app.event(key(KeyCode::F(6)));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("bad:tag".into()));
        app.event(key(KeyCode::Enter));
        assert!(matches!(app.tag_editor, Some(TagEditor::Name(_))));
        assert!(app.status.starts_with("Error:"));
        assert_eq!(app.draft_tags, Config::default().tags);
        app.event(key(KeyCode::Esc));
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("tag".into()));
        app.event(ctrl('s'));
        assert_eq!(Config::load(dir.path())?.tags.len(), 2);
        app.event(key(KeyCode::F(6)));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("tag".into()));
        app.event(key(KeyCode::Enter));
        assert!(matches!(app.tag_editor, Some(TagEditor::Name(_))));
        assert!(app.status.contains("already exists"));
        app.event(key(KeyCode::Esc));
        app.event(key(KeyCode::Delete)); // The "New tag" row is not a stored tag.
        assert_eq!(app.draft_tags.len(), 2);
        app.event(key(KeyCode::Up));
        app.event(key(KeyCode::Delete));
        app.event(ctrl('s'));
        assert_eq!(Config::load(dir.path())?.tags, Config::default().tags);
        Ok(())
    }

    #[test]
    fn daily_tag_cannot_be_deleted_but_its_color_can_be_changed() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        assert!(app.current.as_ref().unwrap().title.starts_with("daily: "));
        assert!(app.content().is_empty());
        app.event(key(KeyCode::F(6)));
        assert_eq!(app.selected_tag().as_deref(), Some(DAILY_TAG));
        assert!(screen(&mut app, 100, 28)?.contains("daily  ● Cyan (built-in)"));
        app.event(key(KeyCode::Delete));
        assert_eq!(app.draft_tags, Config::default().tags);
        assert!(app.status.contains("built in"));
        app.event(key(KeyCode::Enter));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        app.event(ctrl('s'));
        assert_eq!(app.mode, Mode::Edit);
        assert_eq!(Config::load(dir.path())?.tags[DAILY_TAG], Accent::Green);
        assert_eq!(
            app.note_title(&app.current.as_ref().unwrap().title).spans[0]
                .style
                .fg,
            Some(Accent::Green.color())
        );
        app.event(ctrl('d'));
        assert!(app.content().is_empty());
        assert_eq!(app.store.list()?.len(), 1);
        app.event(key(KeyCode::F(6)));
        app.event(key(KeyCode::Delete));
        app.event(ctrl('s'));
        assert_eq!(Config::load(dir.path())?.tags[DAILY_TAG], Accent::Green);
        Ok(())
    }

    #[test]
    fn ui_shows_focus_marker_and_resizes_sidebar() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        store.create("Ideas")?;
        let mut app = App::new(store, false)?;
        let mut positions = Vec::new();
        for (width, height) in [(40, 14), (100, 28), (200, 36)] {
            let text = screen(&mut app, width, height)?;
            for line in if width == 40 { LOGO_SMALL } else { LOGO }.lines() {
                assert!(text.contains(line));
            }
            assert!(text.lines().any(|line| line.starts_with(" LIST  ")));
            assert!(
                text.lines()
                    .last()
                    .unwrap()
                    .trim_end()
                    .ends_with(concat!("v", env!("CARGO_PKG_VERSION")))
            );
            assert!(text.contains("> Ideas"));
            assert!(!text.contains("READ ONLY"));
            assert!(!text.contains("ACTIVE"));
            assert!(
                text.lines()
                    .take(height as usize - 1)
                    .all(|line| !line.contains("Enter edit"))
            );
            let sidebar_end = text
                .lines()
                .next()
                .unwrap()
                .chars()
                .position(|c| c == '┌')
                .unwrap();
            positions.push(sidebar_end);
            let art_height = if width == 40 { 1 } else { 3 };
            for row in [0, art_height + 1] {
                assert!(
                    text.lines()
                        .nth(row)
                        .unwrap()
                        .chars()
                        .take(sidebar_end)
                        .all(|c| c == ' ')
                );
            }
            for row in 1..art_height + 1 {
                let cells: Vec<_> = text
                    .lines()
                    .nth(row)
                    .unwrap()
                    .chars()
                    .take(sidebar_end)
                    .collect();
                assert!(cells[..2].iter().all(|&c| c == ' '));
                assert!(cells[sidebar_end - 2..].iter().all(|&c| c == ' '));
            }
        }
        assert!(positions[0] < positions[1] && positions[1] < positions[2]);
        app.event(key(KeyCode::Enter));
        let editing = screen(&mut app, 100, 28)?;
        assert!(editing.lines().any(|line| line.starts_with(" EDIT  ")));
        assert!(!editing.contains("INACTIVE"));
        assert!(editing.contains("F5 rename"));
        app.event(key(KeyCode::F(4)));
        let settings = screen(&mut app, 100, 28)?;
        assert!(settings.contains("Primary color"));
        assert!(settings.contains("> ● Cyan (saved)"));
        if let Ok(path) = std::env::var("NOTU_UI_DUMP") {
            std::fs::write(
                path,
                format!(
                    "LIST\n{}\n\nEDIT\n{editing}\n\nSETTINGS\n{settings}\n",
                    screen(
                        &mut App::new(Workspace::open(Some(dir.path().to_owned()))?, false)?,
                        100,
                        28
                    )?
                ),
            )?;
        }
        // The brand must survive narrow windows and every dialog overlay.
        for mode in [
            Mode::Browse,
            Mode::Edit,
            Mode::Preview,
            Mode::New,
            Mode::Search,
            Mode::Settings,
            Mode::Rename,
            Mode::Delete,
        ] {
            app.mode = mode;
            for help in [false, true] {
                app.help = help;
                for (width, height, logo) in [
                    (40, 10, LOGO_SMALL),
                    (40, 28, LOGO_INITIAL),
                    (100, 28, LOGO),
                ] {
                    let text = screen(&mut app, width, height)?;
                    for hint in HOTKEYS.split("  ") {
                        assert!(
                            text.contains(hint),
                            "missing {hint} in {mode:?}, {width}x{height}"
                        );
                    }
                    for line in logo.lines() {
                        assert!(
                            text.contains(line),
                            "logo obscured in {mode:?}, help={help}, {width}x{height}"
                        );
                    }
                }
            }
        }
        for (width, height) in [(30, 8), (3, 2)] {
            let text = screen(&mut app, width, height)?;
            for line in LOGO_SMALL.lines() {
                assert!(text.contains(line));
            }
        }
        Ok(())
    }

    #[test]
    fn search_fields_combine_tags_names_and_target_the_selected_note() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.create("work: alpha")?;
        store.save(&first, "", "First body")?;
        let second = store.create("work: beta")?;
        let other = store.create("home: beta")?;
        let mut app = App::new(store, false)?;
        app.event(ctrl('t'));
        app.event(Event::Paste("WORK".into()));
        assert_eq!(app.filtered.len(), 2);
        assert_ne!(app.current.as_ref().unwrap().path, other.path);
        app.event(key(KeyCode::Down));
        assert_eq!(app.current.as_ref().unwrap().path, first.path);
        app.event(key(KeyCode::F(5)));
        assert_eq!(app.title_editor.lines()[0], first.title);
        app.event(key(KeyCode::Esc));
        app.event(ctrl('f'));
        app.event(Event::Paste("bta".into()));
        assert!(app.filtered.is_empty());
        app.event(key(KeyCode::Home));
        app.event(key(KeyCode::Right));
        app.event(Event::Paste("e".into()));
        assert_eq!(app.query, "beta");
        assert_eq!(app.selected().unwrap().path, second.path);
        assert_eq!(app.filtered.len(), 1);
        app.event(key(KeyCode::Enter));
        let screen = screen(&mut app, 100, 28)?;
        assert!(screen.contains("tag: WORK"));
        assert!(screen.contains("name: beta"));
        app.event(key(KeyCode::Esc));
        assert_eq!(app.filtered.len(), 3);
        assert!(app.query.is_empty() && app.tag_filter.is_empty());
        Ok(())
    }

    #[test]
    fn literal_find_is_unicode_aware_wraps_and_does_not_edit_content() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Search")?;
        let body = "Привет [🦀]\nпривет [🦀]\nOther";
        store.save(&note, "", body)?;
        let mut app = App::new(store, false)?;
        app.event(ctrl('g'));
        app.event(Event::Paste("ПРИВЕТ [🦀]".into()));
        assert_eq!(app.editor.cursor(), (0, 0));
        assert!(app.status.starts_with("2 matches"));
        assert_eq!(app.editor.search_style().bg, Some(Color::DarkGray));
        app.event(key(KeyCode::Enter));
        app.event(key(KeyCode::F(9)));
        assert_eq!(app.editor.cursor(), (1, 0));
        app.event(key(KeyCode::F(9)));
        assert_eq!(app.editor.cursor(), (0, 0));
        app.event(key(KeyCode::F(10)));
        assert_eq!(app.editor.cursor(), (1, 0));
        assert!(!app.dirty);
        assert_eq!(app.content(), body);
        app.event(ctrl('g'));
        app.event(Event::Paste("not present".into()));
        assert!(app.status.starts_with("No matches"));
        app.event(key(KeyCode::Esc));
        assert_eq!(app.editor.cursor(), (1, 0));
        assert!(app.editor.search_pattern().is_none());
        assert_eq!(app.store.read(&note)?, body);
        Ok(())
    }

    #[test]
    fn modal_transitions_cancel_deletion_and_restore_keeps_body_and_date() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let note = store.create("Keep me")?;
        store.save(&note, "", "Original body")?;
        let mut app = App::new(store, false)?;
        for command in [
            key(KeyCode::F(4)),
            key(KeyCode::F(6)),
            ctrl('g'),
            ctrl('f'),
            ctrl('t'),
        ] {
            app.browse()?;
            app.event(key(KeyCode::F(7)));
            app.event(command);
            app.event(key(KeyCode::Esc));
            assert_ne!(app.mode, Mode::Delete);
            assert!(app.delete_request.is_none());
            assert!(note.path.exists());
        }
        app.browse()?;
        app.event(key(KeyCode::F(7)));
        app.event(key(KeyCode::Char('y')));
        assert!(app.current.is_none());
        app.event(key(KeyCode::F(8)));
        assert_eq!(app.current.as_ref().unwrap().path, note.path);
        assert_eq!(
            app.current.as_ref().unwrap().created.timestamp_millis(),
            note.created.timestamp_millis()
        );
        assert_eq!(app.content(), "Original body");
        Ok(())
    }

    #[test]
    fn new_tag_can_be_edited_and_cancelled_without_leaking_into_settings() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        app.event(key(KeyCode::F(6)));
        app.event(key(KeyCode::Down));
        app.event(key(KeyCode::Enter));
        app.event(Event::Paste("wrk".into()));
        app.event(key(KeyCode::Home));
        app.event(key(KeyCode::Right));
        app.event(Event::Paste("o".into()));
        app.event(key(KeyCode::Enter));
        assert!(
            matches!(&app.tag_editor, Some(TagEditor::Color { name, new: true, .. }) if name == "work")
        );
        assert!(!app.draft_tags.contains_key("work"));
        app.event(key(KeyCode::Esc));
        app.event(ctrl('s'));
        assert!(!Config::load(dir.path())?.tags.contains_key("work"));
        Ok(())
    }

    #[test]
    fn note_positions_help_scroll_and_preview_pages_follow_viewport() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Workspace::open(Some(dir.path().to_owned()))?;
        let first = store.create("Long")?;
        store.save(
            &first,
            "",
            &(0..100)
                .map(|i| format!("Line {i}\n\n"))
                .collect::<String>(),
        )?;
        let second = store.create("Short")?;
        let mut app = App::new(store, false)?;
        app.open(first.clone(), Mode::Edit)?;
        assert_eq!(app.editor.cursor(), (0, 0));
        app.editor.move_cursor(CursorMove::Jump(60, 3));
        screen(&mut app, 100, 28)?;
        for _ in 0..4 {
            app.editor.move_cursor(CursorMove::Up);
        }
        app.editor.move_cursor(CursorMove::Jump(56, 3));
        screen(&mut app, 100, 28)?;
        let top = app.editor_top;
        app.open(second, Mode::Browse)?;
        app.open(first.clone(), Mode::Edit)?;
        assert_eq!(app.editor.cursor(), (56, 3));
        assert_eq!(app.editor_top, top);
        let restored = screen(&mut app, 100, 28)?;
        assert!(
            restored.lines().nth(1).unwrap().contains("Line 19"),
            "{restored}"
        );
        app.toggle_preview();
        screen(&mut app, 100, 28)?;
        app.event(key(KeyCode::PageDown));
        assert_eq!(app.scroll, app.view_height);
        let mut terminal = Terminal::new(TestBackend::new(100, 28))?;
        terminal.draw(|frame| app.draw(frame))?;
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .any(|cell| cell.symbol() == "┃")
        );
        let scroll = app.scroll;
        app.open(app.store.create("Another")?, Mode::Browse)?;
        app.open(first, Mode::Preview)?;
        assert_eq!(app.scroll, scroll);
        app.event(key(KeyCode::F(1)));
        let first_page = screen(&mut app, 80, 24)?;
        assert!(!first_page.contains("HELP    ↑/↓"));
        app.event(key(KeyCode::End));
        let last_page = screen(&mut app, 80, 24)?;
        assert!(last_page.contains("HELP    ↑/↓"));
        assert!(app.help_scroll > 0);
        app.event(key(KeyCode::Esc));
        assert!(!app.help);
        Ok(())
    }

    #[test]
    fn notifications_do_not_move_panels_and_titles_keep_graphemes_and_color() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        app.edit();
        let find_bottom = |text: &str| {
            text.lines()
                .enumerate()
                .filter(|(_, line)| line.contains('┘'))
                .map(|(row, _)| row)
                .last()
                .unwrap()
        };
        let before = screen(&mut app, 100, 28)?;
        app.status = "Saved".into();
        let saved = screen(&mut app, 100, 28)?;
        app.status = format!("Error: {}", "long message ".repeat(20));
        let error = screen(&mut app, 100, 28)?;
        assert_eq!(find_bottom(&before), find_bottom(&saved));
        assert_eq!(find_bottom(&saved), find_bottom(&error));
        let line = shorten_line(
            Line::from(vec![
                Span::styled("work:", Style::default().fg(Color::Yellow)),
                Span::raw(" 👩‍💻 cafe\u{301} and long title"),
            ]),
            10,
        );
        assert!(line.width() <= 10);
        assert_eq!(line.spans[0].style.fg, Some(Color::Yellow));
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        assert!(text.contains("👩‍💻"));
        assert!(text.ends_with('…'));
        Ok(())
    }

    #[test]
    fn footer_keeps_mode_all_shortcuts_and_version_visible_on_resize() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let mut app = App::new(Workspace::open(Some(dir.path().to_owned()))?, true)?;
        // Include a status message to check it cannot displace the controls.
        app.status = "Saved".into();
        for (mode, label) in [
            (Mode::Browse, "LIST"),
            (Mode::Edit, "EDIT"),
            (Mode::Preview, "PREVIEW"),
            (Mode::Search, "SEARCH"),
            (Mode::Find, "FIND"),
            (Mode::New, "NEW"),
            (Mode::Settings, "SETTINGS"),
            (Mode::Rename, "RENAME"),
            (Mode::Delete, "DELETE"),
        ] {
            app.mode = mode;
            for (width, height) in [
                (200, 24),
                (40, 10),
                (100, 24),
                (30, 12),
                (60, 18),
                (200, 24),
            ] {
                let mut terminal = Terminal::new(TestBackend::new(width, height))?;
                terminal.draw(|frame| app.draw(frame))?;
                let buffer = terminal.backend().buffer();
                let lines: Vec<String> = buffer
                    .content
                    .chunks(width as usize)
                    .map(|row| row.iter().map(|cell| cell.symbol()).collect())
                    .collect();
                let start = lines
                    .iter()
                    .position(|line| line.starts_with(&format!(" {label}  ")))
                    .unwrap();
                let actual = lines[start..]
                    .join(" ")
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let expected = format!("{label} {HOTKEYS} v{}", env!("CARGO_PKG_VERSION"))
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                assert_eq!(
                    actual, expected,
                    "clipped footer in {mode:?}, {width}x{height}"
                );
                for hint in HOTKEYS.split("  ") {
                    assert!(
                        lines[start..].iter().any(|line| line.contains(hint)),
                        "split shortcut {hint}"
                    );
                }
                assert!(
                    lines[start..]
                        .iter()
                        .all(|line| line.starts_with(' ') && line.ends_with(' '))
                );
                assert_eq!(buffer[(1, start as u16)].fg, app.selected_accent().color());
                assert!(buffer[(1, start as u16)].modifier.contains(Modifier::BOLD));
                let version_x = lines
                    .last()
                    .unwrap()
                    .find(concat!("v", env!("CARGO_PKG_VERSION")))
                    .unwrap() as u16;
                assert_eq!(buffer[(version_x, height - 1)].fg, Color::DarkGray);
                assert!(buffer.content.iter().all(|cell| cell.bg != Color::White));
                if width == 200 {
                    assert_eq!(start, height as usize - 1);
                }
            }
        }
        Ok(())
    }
}
