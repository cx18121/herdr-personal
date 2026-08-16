use std::{collections::HashMap, io};

use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, MouseButton,
        MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use ratatui::{
    Frame, Terminal,
    backend::CrosstermBackend,
    buffer::Buffer,
    layout::{Alignment, Constraint, Layout, Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget},
};

use crate::{
    error::AppResult,
    herdr::HerdrClient,
    layout::{LayoutDescription, LayoutNode, SplitDirection},
    operations::{self, LayoutPreset},
};

pub fn run(client: &HerdrClient, pane_id: &str) -> AppResult<()> {
    let layout = client.export_layout(pane_id)?;
    let mut app = Picker::new(layout)?;
    let mut terminal = TerminalSession::start()?;

    loop {
        terminal.terminal.draw(|frame| app.render(frame))?;
        let request = match event::read()? {
            Event::Key(key) if key.kind == KeyEventKind::Press => app.handle_key(key.code),
            Event::Mouse(mouse) => app.handle_mouse(mouse),
            Event::Resize(_, _) => PickerRequest::None,
            _ => PickerRequest::None,
        };

        match request {
            PickerRequest::None => {}
            PickerRequest::Close => break,
            PickerRequest::Activate => {
                if app.is_rearrange_selected() {
                    app.focus_preview();
                    continue;
                }
                if !app.can_apply() {
                    continue;
                }
                app.status = Some(PickerStatus::Applying);
                terminal.terminal.draw(|frame| app.render(frame))?;
                if app.apply_selected(client, pane_id)
                    && let Err(error) = app.refresh(client, pane_id)
                {
                    app.status = Some(PickerStatus::Error(error.to_string()));
                }
            }
            PickerRequest::ApplyLayout(target) => {
                app.status = Some(PickerStatus::Applying);
                terminal.terminal.draw(|frame| app.render(frame))?;
                if app.apply_layout(client, pane_id, target)
                    && let Err(error) = app.refresh(client, pane_id)
                {
                    app.status = Some(PickerStatus::Error(error.to_string()));
                }
            }
        }
    }

    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PickerItem {
    Preset(LayoutPreset),
    Action(PickerAction),
}

impl PickerItem {
    fn label(self) -> &'static str {
        match self {
            Self::Preset(LayoutPreset::Columns) => "Columns",
            Self::Preset(LayoutPreset::Rows) => "Rows",
            Self::Preset(LayoutPreset::Grid) => "Grid",
            Self::Preset(LayoutPreset::Left) => "Left",
            Self::Preset(LayoutPreset::Right) => "Right",
            Self::Preset(LayoutPreset::Top) => "Top",
            Self::Preset(LayoutPreset::Bottom) => "Bottom",
            Self::Action(action) => action.label(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PickerAction {
    Rearrange,
    Expand,
    Balance,
    Rotate,
    Undo,
}

impl PickerAction {
    fn label(self) -> &'static str {
        match self {
            Self::Rearrange => "Rearrange",
            Self::Expand => "Expand",
            Self::Balance => "Balance",
            Self::Rotate => "Rotate",
            Self::Undo => "Undo",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Rearrange => "Drag a numbered pane",
            Self::Expand => "Expand the focused pane",
            Self::Balance => "Balance the focused split",
            Self::Rotate => "Rotate the focused split",
            Self::Undo => "Restore the previous layout",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum PickerTab {
    Actions,
    Layouts,
}

impl PickerTab {
    fn label(self) -> &'static str {
        match self {
            Self::Actions => "Actions",
            Self::Layouts => "Layouts",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Actions => Self::Layouts,
            Self::Layouts => Self::Actions,
        }
    }
}

enum PickerRequest {
    None,
    Close,
    Activate,
    ApplyLayout(LayoutNode),
}

struct Picker {
    layout: LayoutDescription,
    pane_numbers: HashMap<String, usize>,
    items: Vec<PickerItem>,
    selected: usize,
    active_tab: PickerTab,
    last_action: usize,
    last_layout: usize,
    current_preset: Option<usize>,
    undo_target: Option<LayoutNode>,
    unavailable: Option<&'static str>,
    tab_areas: Vec<(PickerTab, Rect)>,
    row_areas: Vec<(usize, Rect)>,
    pane_areas: Vec<(String, Rect)>,
    armed_row: Option<usize>,
    mouse_position: Option<(u16, u16)>,
    preview_focused: bool,
    keyboard_pane: usize,
    hovered_pane: Option<String>,
    drag_source: Option<String>,
    drop_target: Option<DropTarget>,
    status: Option<PickerStatus>,
}

enum PickerStatus {
    Applying,
    Error(String),
}

#[derive(Clone, PartialEq)]
struct DropTarget {
    pane_id: String,
    zone: DropZone,
}

#[derive(Clone, Copy, PartialEq)]
enum DropZone {
    Center,
    Left,
    Right,
    Top,
    Bottom,
}

impl DropZone {
    fn next(self) -> Self {
        match self {
            Self::Center => Self::Left,
            Self::Left => Self::Right,
            Self::Right => Self::Top,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Center,
        }
    }
}

impl Picker {
    fn new(layout: LayoutDescription) -> AppResult<Self> {
        let pane_count = layout.root.pane_count()?;
        let unavailable = if layout.zoomed {
            Some("Unzoom this tab to change its layout.")
        } else if pane_count < 2 {
            Some("Add another pane to arrange a layout.")
        } else if pane_count > 4 {
            Some("Arrange works with two to four panes.")
        } else {
            None
        };
        let items = if unavailable.is_none() {
            [
                PickerAction::Rearrange,
                PickerAction::Expand,
                PickerAction::Balance,
                PickerAction::Rotate,
                PickerAction::Undo,
            ]
            .into_iter()
            .map(PickerItem::Action)
            .chain(
                LayoutPreset::available(pane_count)
                    .into_iter()
                    .map(PickerItem::Preset),
            )
            .collect()
        } else {
            Vec::new()
        };
        let selected = 0;
        let current_preset = Self::current_preset_for(&items, &layout)?;
        let last_layout = current_preset.unwrap_or_else(|| {
            items
                .iter()
                .position(|item| matches!(item, PickerItem::Preset(_)))
                .unwrap_or_default()
        });
        let undo_target = operations::undo_target(&layout).unwrap_or(None);
        let pane_ids = layout.root.pane_ids()?;
        let keyboard_pane = pane_ids
            .iter()
            .position(|pane_id| pane_id == &layout.focused_pane_id)
            .unwrap_or(0);
        let pane_numbers = pane_ids
            .into_iter()
            .enumerate()
            .map(|(index, pane_id)| (pane_id, index + 1))
            .collect();

        Ok(Self {
            layout,
            pane_numbers,
            items,
            selected,
            active_tab: PickerTab::Actions,
            last_action: selected,
            last_layout,
            current_preset,
            undo_target,
            unavailable,
            tab_areas: Vec::new(),
            row_areas: Vec::new(),
            pane_areas: Vec::new(),
            armed_row: None,
            mouse_position: None,
            preview_focused: false,
            keyboard_pane,
            hovered_pane: None,
            drag_source: None,
            drop_target: None,
            status: None,
        })
    }

    fn current_preset_for(
        items: &[PickerItem],
        layout: &LayoutDescription,
    ) -> AppResult<Option<usize>> {
        let normalized = layout.root.normalized()?;
        Ok(items.iter().enumerate().find_map(|(index, item)| {
            let PickerItem::Preset(preset) = item else {
                return None;
            };
            operations::target_for_preset(layout, *preset)
                .and_then(|target| target.normalized())
                .is_ok_and(|target| target == normalized)
                .then_some(index)
        }))
    }

    fn refresh(&mut self, client: &HerdrClient, pane_id: &str) -> AppResult<()> {
        let keep_keyboard_focus = self.preview_focused && self.is_rearrange_selected();
        let layout = client.export_layout(pane_id)?;
        let current_preset = Self::current_preset_for(&self.items, &layout)?;
        let undo_target = operations::undo_target(&layout)?;
        let pane_ids = layout.root.pane_ids()?;
        let keyboard_pane = pane_ids
            .iter()
            .position(|candidate| candidate == &layout.focused_pane_id)
            .unwrap_or_default();

        self.ensure_pane_numbers(&pane_ids);
        self.layout = layout;
        self.current_preset = current_preset;
        if let Some(index) = current_preset {
            self.last_layout = index;
        }
        self.undo_target = undo_target;
        self.keyboard_pane = keyboard_pane;
        self.preview_focused = keep_keyboard_focus;
        self.hovered_pane = None;
        self.drag_source = None;
        self.drop_target = None;
        self.armed_row = None;
        self.status = None;
        if !self.item_enabled(self.selected) {
            self.move_selection(-1);
        }
        Ok(())
    }

    fn ensure_pane_numbers(&mut self, pane_ids: &[String]) {
        let mut next = self
            .pane_numbers
            .values()
            .copied()
            .max()
            .unwrap_or_default()
            + 1;
        for pane_id in pane_ids {
            if !self.pane_numbers.contains_key(pane_id) {
                self.pane_numbers.insert(pane_id.clone(), next);
                next += 1;
            }
        }
    }

    fn handle_key(&mut self, key: KeyCode) -> PickerRequest {
        if self.preview_focused && self.is_rearrange_selected() {
            return self.handle_rearrange_key(key);
        }

        match key {
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.switch_tab();
                PickerRequest::None
            }
            KeyCode::Up => {
                self.move_selection(-1);
                PickerRequest::None
            }
            KeyCode::Down => {
                self.move_selection(1);
                PickerRequest::None
            }
            KeyCode::Enter if self.can_apply() => PickerRequest::Activate,
            KeyCode::Esc => PickerRequest::Close,
            _ => PickerRequest::None,
        }
    }

    fn handle_rearrange_key(&mut self, key: KeyCode) -> PickerRequest {
        let pane_ids = self.layout.root.pane_ids().unwrap_or_default();
        match key {
            KeyCode::Left | KeyCode::Up => {
                if !pane_ids.is_empty() {
                    self.keyboard_pane = if self.keyboard_pane == 0 {
                        pane_ids.len() - 1
                    } else {
                        self.keyboard_pane - 1
                    };
                    self.update_keyboard_target(&pane_ids);
                }
                PickerRequest::None
            }
            KeyCode::Right | KeyCode::Down => {
                if !pane_ids.is_empty() {
                    self.keyboard_pane = (self.keyboard_pane + 1) % pane_ids.len();
                    self.update_keyboard_target(&pane_ids);
                }
                PickerRequest::None
            }
            KeyCode::Tab if self.drag_source.is_some() => {
                if let Some(target) = &mut self.drop_target {
                    target.zone = target.zone.next();
                }
                PickerRequest::None
            }
            KeyCode::Char(' ') => {
                let Some(pane_id) = pane_ids.get(self.keyboard_pane).cloned() else {
                    return PickerRequest::None;
                };
                if self.drag_source.is_none() {
                    self.drag_source = Some(pane_id);
                    self.move_keyboard_to_other_pane(&pane_ids);
                    self.update_keyboard_target(&pane_ids);
                    PickerRequest::None
                } else {
                    self.target_layout()
                        .map_or(PickerRequest::None, PickerRequest::ApplyLayout)
                }
            }
            KeyCode::Esc => {
                if self.drag_source.is_some() {
                    self.drag_source = None;
                    self.drop_target = None;
                    self.status = None;
                } else {
                    self.cancel_rearrange();
                }
                PickerRequest::None
            }
            _ => PickerRequest::None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) -> PickerRequest {
        let tab_hit = self
            .tab_areas
            .iter()
            .find(|(_, area)| area.contains((mouse.column, mouse.row).into()))
            .map(|(tab, _)| *tab);
        if mouse.kind == MouseEventKind::Down(MouseButton::Left)
            && let Some(tab) = tab_hit
        {
            self.switch_to_tab(tab);
            return PickerRequest::None;
        }

        let over_preview = self.pane_at(mouse.column, mouse.row).is_some();
        if over_preview && !self.is_rearrange_selected() {
            self.switch_to_tab(PickerTab::Actions);
            self.select(0);
        }
        if self.is_rearrange_selected()
            && let Some(request) = self.handle_rearrange_mouse(mouse)
        {
            return request;
        }

        let hit = self
            .row_areas
            .iter()
            .find(|(_, area)| area.contains((mouse.column, mouse.row).into()))
            .map(|(index, _)| *index);

        match mouse.kind {
            MouseEventKind::Moved => {
                let position = (mouse.column, mouse.row);
                let has_moved = self
                    .mouse_position
                    .replace(position)
                    .is_some_and(|previous| previous != position);
                if has_moved
                    && let Some(index) = hit
                    && index != self.selected
                    && self.item_enabled(index)
                {
                    self.select(index);
                }
                PickerRequest::None
            }
            MouseEventKind::Down(MouseButton::Left) => {
                self.armed_row = hit.filter(|index| self.item_enabled(*index));
                if let Some(index) = self.armed_row
                    && index != self.selected
                {
                    self.select(index);
                }
                PickerRequest::None
            }
            MouseEventKind::Up(MouseButton::Left) => {
                let activate = hit.is_some() && hit == self.armed_row;
                self.armed_row = None;
                if activate {
                    PickerRequest::Activate
                } else {
                    PickerRequest::None
                }
            }
            _ => PickerRequest::None,
        }
    }

    fn handle_rearrange_mouse(&mut self, mouse: MouseEvent) -> Option<PickerRequest> {
        let pane = self.pane_at(mouse.column, mouse.row);
        match mouse.kind {
            MouseEventKind::Moved if self.drag_source.is_none() => {
                self.hovered_pane = pane.as_ref().map(|(pane_id, _)| pane_id.clone());
                self.hovered_pane.as_ref().map(|_| PickerRequest::None)
            }
            MouseEventKind::Down(MouseButton::Left) => {
                let (pane_id, _) = pane?;
                self.hovered_pane = None;
                self.drag_source = Some(pane_id);
                self.drop_target = None;
                Some(PickerRequest::None)
            }
            MouseEventKind::Drag(MouseButton::Left) | MouseEventKind::Moved
                if self.drag_source.is_some() =>
            {
                self.hovered_pane = None;
                self.drop_target = pane.and_then(|(pane_id, area)| {
                    (Some(&pane_id) != self.drag_source.as_ref()).then(|| DropTarget {
                        pane_id,
                        zone: drop_zone(area, mouse.column, mouse.row),
                    })
                });
                Some(PickerRequest::None)
            }
            MouseEventKind::Up(MouseButton::Left) if self.drag_source.is_some() => {
                let request = self
                    .target_layout()
                    .map_or(PickerRequest::None, PickerRequest::ApplyLayout);
                if matches!(request, PickerRequest::None) {
                    self.drag_source = None;
                    self.drop_target = None;
                    self.hovered_pane = pane.map(|(pane_id, _)| pane_id);
                }
                Some(request)
            }
            _ => None,
        }
    }

    fn apply_selected(&mut self, client: &HerdrClient, pane_id: &str) -> bool {
        let Some(item) = self.items.get(self.selected).copied() else {
            return false;
        };
        let result = match item {
            PickerItem::Preset(preset) => operations::apply_preset(client, pane_id, preset),
            PickerItem::Action(PickerAction::Expand) => operations::expand(client, pane_id),
            PickerItem::Action(PickerAction::Balance) => operations::balance(client, pane_id),
            PickerItem::Action(PickerAction::Rotate) => operations::rotate(client, pane_id),
            PickerItem::Action(PickerAction::Undo) => {
                operations::undo(client, pane_id).and_then(|changed| {
                    changed
                        .then_some(())
                        .ok_or_else(|| crate::error::AppError::Message("Nothing to undo".into()))
                })
            }
            PickerItem::Action(PickerAction::Rearrange) => return false,
        };

        match result {
            Ok(()) => true,
            Err(error) => {
                self.status = Some(PickerStatus::Error(error.to_string()));
                false
            }
        }
    }

    fn apply_layout(&mut self, client: &HerdrClient, pane_id: &str, target: LayoutNode) -> bool {
        match operations::apply_layout(client, pane_id, target) {
            Ok(()) => true,
            Err(error) => {
                self.status = Some(PickerStatus::Error(error.to_string()));
                false
            }
        }
    }

    fn move_selection(&mut self, amount: isize) {
        let indices = self.indices_for_tab(self.active_tab);
        let Some(position) = indices.iter().position(|index| *index == self.selected) else {
            return;
        };
        let len = indices.len() as isize;
        let mut next = position as isize;
        for _ in 0..indices.len() {
            next = (next + amount).rem_euclid(len);
            let index = indices[next as usize];
            if self.item_enabled(index) {
                self.select(index);
                break;
            }
        }
    }

    fn indices_for_tab(&self, tab: PickerTab) -> Vec<usize> {
        self.items
            .iter()
            .enumerate()
            .filter_map(|(index, item)| match (tab, item) {
                (PickerTab::Actions, PickerItem::Action(_))
                | (PickerTab::Layouts, PickerItem::Preset(_)) => Some(index),
                _ => None,
            })
            .collect()
    }

    fn switch_tab(&mut self) {
        self.switch_to_tab(self.active_tab.other());
    }

    fn switch_to_tab(&mut self, tab: PickerTab) {
        if self.active_tab == tab {
            return;
        }
        self.active_tab = tab;
        let index = match tab {
            PickerTab::Actions => self.last_action,
            PickerTab::Layouts => self.last_layout,
        };
        self.select(index);
    }

    fn select(&mut self, index: usize) {
        self.selected = index;
        match self.items.get(index) {
            Some(PickerItem::Action(_)) => self.last_action = index,
            Some(PickerItem::Preset(_)) => self.last_layout = index,
            None => {}
        }
        self.preview_focused = false;
        self.hovered_pane = None;
        self.drag_source = None;
        self.drop_target = None;
        self.status = None;
    }

    fn item_enabled(&self, index: usize) -> bool {
        !matches!(
            self.items.get(index),
            Some(PickerItem::Action(PickerAction::Undo)) if self.undo_target.is_none()
        )
    }

    fn can_apply(&self) -> bool {
        !self.items.is_empty() && self.item_enabled(self.selected)
    }

    fn is_rearrange_selected(&self) -> bool {
        matches!(
            self.items.get(self.selected),
            Some(PickerItem::Action(PickerAction::Rearrange))
        )
    }

    fn focus_preview(&mut self) {
        self.preview_focused = true;
        self.hovered_pane = None;
        self.drag_source = None;
        self.drop_target = None;
        self.status = None;
    }

    fn cancel_rearrange(&mut self) {
        self.preview_focused = false;
        self.hovered_pane = None;
        self.drag_source = None;
        self.drop_target = None;
        self.status = None;
    }

    fn move_keyboard_to_other_pane(&mut self, pane_ids: &[String]) {
        if pane_ids.len() < 2 {
            return;
        }
        for _ in 0..pane_ids.len() {
            self.keyboard_pane = (self.keyboard_pane + 1) % pane_ids.len();
            if Some(&pane_ids[self.keyboard_pane]) != self.drag_source.as_ref() {
                break;
            }
        }
    }

    fn update_keyboard_target(&mut self, pane_ids: &[String]) {
        let Some(source) = self.drag_source.as_ref() else {
            return;
        };
        let Some(target) = pane_ids.get(self.keyboard_pane) else {
            return;
        };
        if target != source {
            let zone = self
                .drop_target
                .as_ref()
                .map_or(DropZone::Center, |target| target.zone);
            self.drop_target = Some(DropTarget {
                pane_id: target.clone(),
                zone,
            });
        }
    }

    fn pane_at(&self, x: u16, y: u16) -> Option<(String, Rect)> {
        self.pane_areas
            .iter()
            .find(|(_, area)| area.contains((x, y).into()))
            .cloned()
    }

    fn target_layout(&self) -> Option<LayoutNode> {
        let source = self.drag_source.as_deref()?;
        let target = self.drop_target.as_ref()?;
        match target.zone {
            DropZone::Center => self.layout.root.swap_panes(source, &target.pane_id).ok(),
            DropZone::Left => self
                .layout
                .root
                .dock_pane(source, &target.pane_id, SplitDirection::Right, true)
                .ok(),
            DropZone::Right => self
                .layout
                .root
                .dock_pane(source, &target.pane_id, SplitDirection::Right, false)
                .ok(),
            DropZone::Top => self
                .layout
                .root
                .dock_pane(source, &target.pane_id, SplitDirection::Down, true)
                .ok(),
            DropZone::Bottom => self
                .layout
                .root
                .dock_pane(source, &target.pane_id, SplitDirection::Down, false)
                .ok(),
        }
    }

    fn preview_target(&self) -> AppResult<LayoutNode> {
        let Some(item) = self.items.get(self.selected).copied() else {
            return Ok(self.layout.root.clone());
        };
        match item {
            PickerItem::Preset(preset) => operations::target_for_preset(&self.layout, preset),
            PickerItem::Action(PickerAction::Rearrange) => Ok(self.layout.root.clone()),
            PickerItem::Action(PickerAction::Expand) => operations::target_for_expand(&self.layout),
            PickerItem::Action(PickerAction::Balance) => {
                operations::target_for_balance(&self.layout)
            }
            PickerItem::Action(PickerAction::Rotate) => operations::target_for_rotate(&self.layout),
            PickerItem::Action(PickerAction::Undo) => Ok(self
                .undo_target
                .clone()
                .unwrap_or_else(|| self.layout.root.clone())),
        }
    }

    fn preview_label(&self, numbers: &HashMap<String, usize>) -> String {
        if self.is_rearrange_selected() {
            if let (Some(source), Some(target)) = (&self.drag_source, &self.drop_target) {
                let source = numbers.get(source).copied().unwrap_or_default();
                let target_number = numbers.get(&target.pane_id).copied().unwrap_or_default();
                return match target.zone {
                    DropZone::Center => format!("Swap {source} and {target_number}"),
                    DropZone::Left => format!("Move {source} left of {target_number}"),
                    DropZone::Right => format!("Move {source} right of {target_number}"),
                    DropZone::Top => format!("Move {source} above {target_number}"),
                    DropZone::Bottom => format!("Move {source} below {target_number}"),
                };
            }
            if let Some(source) = &self.drag_source {
                let source = numbers.get(source).copied().unwrap_or_default();
                return format!("Choose where to place pane {source}");
            }
            if self.preview_focused {
                return "Choose a pane to move".into();
            }
            if let Some(hovered) = &self.hovered_pane {
                let pane = numbers.get(hovered).copied().unwrap_or_default();
                return format!("Drag pane {pane}");
            }
            return "Drag a pane to move or swap it".into();
        }

        match self.items.get(self.selected) {
            Some(PickerItem::Preset(_)) if self.current_preset == Some(self.selected) => {
                "Current layout".into()
            }
            Some(PickerItem::Preset(preset)) => preset.description().into(),
            Some(PickerItem::Action(PickerAction::Expand)) if self.preview_matches_current() => {
                "Already expanded".into()
            }
            Some(PickerItem::Action(PickerAction::Balance)) if self.preview_matches_current() => {
                "Already balanced".into()
            }
            Some(PickerItem::Action(action)) => action.description().into(),
            None => String::new(),
        }
    }

    fn preview_matches_current(&self) -> bool {
        let target = self.preview_target().and_then(|target| target.normalized());
        let current = self.layout.root.normalized();
        matches!((target, current), (Ok(target), Ok(current)) if target == current)
    }

    fn render(&mut self, frame: &mut Frame<'_>) {
        let frame_area = frame.area().inner(Margin::new(2, 0));
        let vertical = Layout::vertical([
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ]);
        let [tabs_area, main_area, footer_area] = vertical.areas(frame_area);

        if let Some(message) = self.unavailable {
            self.tab_areas.clear();
            self.row_areas.clear();
            self.pane_areas.clear();
            let message_area = centered_rect(main_area, 42, 3);
            frame.render_widget(
                Paragraph::new(message)
                    .alignment(Alignment::Center)
                    .style(Style::default().fg(Color::Gray)),
                message_area,
            );
        } else {
            self.render_tabs(frame, tabs_area);
            let horizontal = Layout::horizontal([
                Constraint::Length(22),
                Constraint::Length(3),
                Constraint::Min(28),
            ]);
            let [list_area, _, preview_area] = horizontal.areas(main_area);
            self.render_list(frame, list_area);
            self.render_preview(frame, preview_area);
        }

        self.render_footer(frame, footer_area);
    }

    fn render_tabs(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.tab_areas.clear();
        let mut x = area.x;
        for tab in [PickerTab::Actions, PickerTab::Layouts] {
            let width = tab.label().len() as u16 + 4;
            let tab_area = Rect::new(x, area.y, width, 1);
            let style = if tab == self.active_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            frame.render_widget(
                Paragraph::new(format!("  {}  ", tab.label())).style(style),
                tab_area,
            );
            self.tab_areas.push((tab, tab_area));
            x += width + 2;
        }
        frame.render_widget(
            Paragraph::new("─".repeat(area.width as usize))
                .style(Style::default().fg(Color::DarkGray)),
            Rect::new(area.x, area.y + 2, area.width, 1),
        );
    }

    fn render_list(&mut self, frame: &mut Frame<'_>, area: Rect) {
        self.row_areas.clear();
        let mut row = area.y + 1;
        match self.active_tab {
            PickerTab::Actions => {
                let adjust = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        matches!(item, PickerItem::Action(action) if *action != PickerAction::Undo)
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                let history = self
                    .items
                    .iter()
                    .position(|item| *item == PickerItem::Action(PickerAction::Undo))
                    .into_iter()
                    .collect::<Vec<_>>();
                row = self.render_list_group(frame, area, row, "Adjust", &adjust);
                row += 1;
                self.render_list_group(frame, area, row, "History", &history);
            }
            PickerTab::Layouts => {
                let equal = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        matches!(item, PickerItem::Preset(preset) if preset.is_equal())
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                let focused = self
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        matches!(item, PickerItem::Preset(preset) if !preset.is_equal())
                            .then_some(index)
                    })
                    .collect::<Vec<_>>();
                row = self.render_list_group(frame, area, row, "Equal", &equal);
                row += 1;
                self.render_list_group(frame, area, row, "Focused", &focused);
            }
        }
    }

    fn render_list_group(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        mut row: u16,
        label: &str,
        indices: &[usize],
    ) -> u16 {
        frame.render_widget(
            Paragraph::new(label).style(
                Style::default()
                    .fg(Color::Gray)
                    .add_modifier(Modifier::BOLD),
            ),
            Rect::new(area.x, row, area.width, 1),
        );
        row += 1;
        for index in indices {
            self.render_list_item(frame, *index, Rect::new(area.x, row, area.width, 1));
            row += 1;
        }
        row
    }

    fn render_list_item(&mut self, frame: &mut Frame<'_>, index: usize, area: Rect) {
        let selected = index == self.selected;
        let enabled = self.item_enabled(index);
        let style = if !enabled {
            Style::default().fg(Color::DarkGray)
        } else if selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if selected { "▸" } else { " " };
        frame.render_widget(
            Paragraph::new(format!("{marker} {}", self.items[index].label())).style(style),
            area,
        );
        if let Some(hint) = self.item_hint(index) {
            let hint_style = if enabled {
                Style::default().fg(Color::Gray)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            frame.render_widget(
                Paragraph::new(hint)
                    .alignment(Alignment::Right)
                    .style(hint_style),
                area,
            );
        }
        self.row_areas.push((index, area));
    }

    fn item_hint(&self, index: usize) -> Option<&'static str> {
        if self.current_preset == Some(index) {
            return Some("current");
        }
        match self.items.get(index) {
            Some(PickerItem::Action(PickerAction::Expand)) => Some("⌘⌥↩"),
            Some(PickerItem::Action(PickerAction::Balance)) => Some("⌘⌥B"),
            Some(PickerItem::Action(PickerAction::Rotate)) => Some("⌘⌥R"),
            Some(PickerItem::Action(PickerAction::Undo)) => Some("⌘⌥Z"),
            _ => None,
        }
    }

    fn render_preview(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let Ok(target) = self.preview_target() else {
            return;
        };
        let label_area = Rect::new(area.x, area.bottom().saturating_sub(2), area.width, 1);
        let diagram_bounds = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(3));
        let diagram_area = centered_rect(diagram_bounds, 36, 11);
        let numbers = self.pane_numbers.clone();
        let rearranging = self.is_rearrange_selected();
        let displayed = if rearranging {
            &self.layout.root
        } else {
            &target
        };
        let inner = diagram_area.inner(Margin::new(1, 1));
        self.pane_areas.clear();
        collect_pane_areas(displayed, inner, &mut self.pane_areas);
        let keyboard_pane = self
            .layout
            .root
            .pane_ids()
            .unwrap_or_default()
            .get(self.keyboard_pane)
            .cloned();
        let highlighted_pane = self.hovered_pane.as_deref().or_else(|| {
            (rearranging && self.preview_focused && self.drag_source.is_none())
                .then_some(keyboard_pane.as_deref())
                .flatten()
        });

        frame.render_widget(
            LayoutPreview {
                root: displayed,
                numbers: &numbers,
                focused_pane_id: &self.layout.focused_pane_id,
                highlighted_pane_id: highlighted_pane,
                source_pane_id: self.drag_source.as_deref(),
                drop_target: self.drop_target.as_ref(),
                pane_areas: &self.pane_areas,
            },
            diagram_area,
        );
        frame.render_widget(
            Paragraph::new(self.preview_label(&numbers))
                .alignment(Alignment::Center)
                .style(Style::default().fg(Color::Gray)),
            label_area,
        );
    }

    fn render_footer(&self, frame: &mut Frame<'_>, area: Rect) {
        let line = match &self.status {
            Some(PickerStatus::Applying) => Line::styled(
                "Applying…",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Some(PickerStatus::Error(error)) => {
                Line::styled(error.as_str(), Style::default().fg(Color::Red))
            }
            None if self.unavailable.is_some() => footer_line(&[("esc", "close")]),
            None if self.preview_focused && self.drag_source.is_some() => footer_line(&[
                ("arrows", "target"),
                ("tab", "position"),
                ("space", "place"),
                ("esc", "back"),
            ]),
            None if self.preview_focused => {
                footer_line(&[("arrows", "pane"), ("space", "pick up"), ("esc", "back")])
            }
            None if self.is_rearrange_selected() => footer_line(&[
                ("drag", "panes"),
                ("tab", "switch"),
                ("enter", "keyboard"),
                ("esc", "close"),
            ]),
            None => footer_line(&[
                ("↑↓", "choose"),
                ("tab", "switch"),
                ("enter", "apply"),
                ("esc", "close"),
            ]),
        };
        frame.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
    }
}

fn footer_line(items: &[(&str, &str)]) -> Line<'static> {
    let spans = items
        .iter()
        .enumerate()
        .flat_map(|(index, (key, action))| {
            let gap = (index > 0).then(|| Span::raw("    "));
            gap.into_iter().chain([
                Span::styled(
                    (*key).to_string(),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!(" {action}"), Style::default().fg(Color::Gray)),
            ])
        })
        .collect::<Vec<_>>();
    Line::from(spans)
}

struct LayoutPreview<'a> {
    root: &'a LayoutNode,
    numbers: &'a HashMap<String, usize>,
    focused_pane_id: &'a str,
    highlighted_pane_id: Option<&'a str>,
    source_pane_id: Option<&'a str>,
    drop_target: Option<&'a DropTarget>,
    pane_areas: &'a [(String, Rect)],
}

impl Widget for LayoutPreview<'_> {
    fn render(self, area: Rect, buffer: &mut Buffer) {
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray))
            .render(area, buffer);
        let inner = area.inner(Margin::new(1, 1));
        render_layout_node(
            self.root,
            inner,
            buffer,
            self.numbers,
            self.focused_pane_id,
            self.highlighted_pane_id,
            self.source_pane_id,
        );

        if let Some(target) = self.drop_target
            && let Some((_, area)) = self
                .pane_areas
                .iter()
                .find(|(pane_id, _)| pane_id == &target.pane_id)
        {
            let highlight = zone_rect(*area, target.zone);
            (highlight.y..highlight.bottom()).for_each(|y| {
                (highlight.x..highlight.right()).for_each(|x| {
                    buffer[(x, y)].set_bg(Color::DarkGray);
                });
            });
        }
    }
}

fn render_layout_node(
    node: &LayoutNode,
    area: Rect,
    buffer: &mut Buffer,
    numbers: &HashMap<String, usize>,
    focused_pane_id: &str,
    highlighted_pane_id: Option<&str>,
    source_pane_id: Option<&str>,
) {
    match node {
        LayoutNode::Pane { pane_id, .. } => {
            let Some(pane_id) = pane_id else {
                return;
            };
            let number = numbers.get(pane_id).copied().unwrap_or_default();
            let source = source_pane_id == Some(pane_id.as_str());
            let label = if source {
                format!("[{number}]")
            } else {
                number.to_string()
            };
            let x = area.x + area.width.saturating_sub(label.len() as u16) / 2;
            let y = area.y + area.height.saturating_sub(1) / 2;
            let highlighted = highlighted_pane_id == Some(pane_id.as_str());
            let style = if source {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::REVERSED)
            } else if highlighted {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
            } else if pane_id == focused_pane_id {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            };
            buffer.set_string(x, y, label, style);
        }
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let has_room = match direction {
                SplitDirection::Right => area.width >= 3,
                SplitDirection::Down => area.height >= 3,
            };
            if !has_room {
                return;
            }

            let (first_area, second_area, divider) = split_rect(area, *direction, *ratio);
            let divider_style = Style::default().fg(Color::DarkGray);
            match direction {
                SplitDirection::Right => {
                    (divider.y..divider.bottom()).for_each(|y| {
                        buffer[(divider.x, y)]
                            .set_char('│')
                            .set_style(divider_style);
                    });
                }
                SplitDirection::Down => {
                    (divider.x..divider.right()).for_each(|x| {
                        buffer[(x, divider.y)]
                            .set_char('─')
                            .set_style(divider_style);
                    });
                }
            }
            render_layout_node(
                first,
                first_area,
                buffer,
                numbers,
                focused_pane_id,
                highlighted_pane_id,
                source_pane_id,
            );
            render_layout_node(
                second,
                second_area,
                buffer,
                numbers,
                focused_pane_id,
                highlighted_pane_id,
                source_pane_id,
            );
        }
    }
}

fn collect_pane_areas(node: &LayoutNode, area: Rect, areas: &mut Vec<(String, Rect)>) {
    match node {
        LayoutNode::Pane {
            pane_id: Some(pane_id),
            ..
        } => areas.push((pane_id.clone(), area)),
        LayoutNode::Pane { .. } => {}
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let has_room = match direction {
                SplitDirection::Right => area.width >= 3,
                SplitDirection::Down => area.height >= 3,
            };
            if !has_room {
                return;
            }
            let (first_area, second_area, _) = split_rect(area, *direction, *ratio);
            collect_pane_areas(first, first_area, areas);
            collect_pane_areas(second, second_area, areas);
        }
    }
}

fn split_rect(area: Rect, direction: SplitDirection, ratio: f64) -> (Rect, Rect, Rect) {
    match direction {
        SplitDirection::Right => {
            let usable = area.width.saturating_sub(1);
            let first_width =
                ((usable as f64 * ratio).round() as u16).clamp(1, usable.saturating_sub(1));
            let second_width = usable.saturating_sub(first_width);
            (
                Rect::new(area.x, area.y, first_width, area.height),
                Rect::new(area.x + first_width + 1, area.y, second_width, area.height),
                Rect::new(area.x + first_width, area.y, 1, area.height),
            )
        }
        SplitDirection::Down => {
            let usable = area.height.saturating_sub(1);
            let first_height =
                ((usable as f64 * ratio).round() as u16).clamp(1, usable.saturating_sub(1));
            let second_height = usable.saturating_sub(first_height);
            (
                Rect::new(area.x, area.y, area.width, first_height),
                Rect::new(area.x, area.y + first_height + 1, area.width, second_height),
                Rect::new(area.x, area.y + first_height, area.width, 1),
            )
        }
    }
}

fn drop_zone(area: Rect, x: u16, y: u16) -> DropZone {
    let width = area.width.max(1) as f64;
    let height = area.height.max(1) as f64;
    let distances = [
        (DropZone::Left, (x.saturating_sub(area.x)) as f64 / width),
        (
            DropZone::Right,
            (area.right().saturating_sub(1).saturating_sub(x)) as f64 / width,
        ),
        (DropZone::Top, (y.saturating_sub(area.y)) as f64 / height),
        (
            DropZone::Bottom,
            (area.bottom().saturating_sub(1).saturating_sub(y)) as f64 / height,
        ),
    ];
    let (zone, distance) = distances
        .into_iter()
        .min_by(|(_, first), (_, second)| first.total_cmp(second))
        .unwrap_or((DropZone::Center, 1.0));
    if distance <= 0.25 {
        zone
    } else {
        DropZone::Center
    }
}

fn zone_rect(area: Rect, zone: DropZone) -> Rect {
    let quarter_width = (area.width / 4).max(1);
    let quarter_height = (area.height / 4).max(1);
    match zone {
        DropZone::Left => Rect::new(area.x, area.y, quarter_width, area.height),
        DropZone::Right => Rect::new(
            area.right().saturating_sub(quarter_width),
            area.y,
            quarter_width,
            area.height,
        ),
        DropZone::Top => Rect::new(area.x, area.y, area.width, quarter_height),
        DropZone::Bottom => Rect::new(
            area.x,
            area.bottom().saturating_sub(quarter_height),
            area.width,
            quarter_height,
        ),
        DropZone::Center => {
            let width = (area.width / 2).max(1);
            let height = (area.height / 2).max(1);
            Rect::new(
                area.x + area.width.saturating_sub(width) / 2,
                area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            )
        }
    }
}

fn centered_rect(area: Rect, max_width: u16, max_height: u16) -> Rect {
    let width = area.width.min(max_width);
    let height = area.height.min(max_height);
    Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<io::Stdout>>,
}

impl TerminalSession {
    fn start() -> AppResult<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        if let Err(error) = execute!(stdout, EnterAlternateScreen, EnableMouseCapture) {
            let _ = disable_raw_mode();
            return Err(error.into());
        }

        match Terminal::new(CrosstermBackend::new(stdout)) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(error) => {
                let mut stdout = io::stdout();
                let _ = execute!(stdout, DisableMouseCapture, LeaveAlternateScreen);
                let _ = disable_raw_mode();
                Err(error.into())
            }
        }
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            DisableMouseCapture,
            LeaveAlternateScreen
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout(root: LayoutNode, focused_pane_id: &str) -> LayoutDescription {
        LayoutDescription {
            workspace_id: "w1".into(),
            tab_id: "t1".into(),
            zoomed: false,
            focused_pane_id: focused_pane_id.into(),
            root,
        }
    }

    #[test]
    fn one_pane_shows_guidance_instead_of_failing() {
        let picker = Picker::new(layout(LayoutNode::pane("p1".into()), "p1"))
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            picker.unavailable,
            Some("Add another pane to arrange a layout.")
        );
        assert!(picker.items.is_empty());
        assert!(!picker.can_apply());
    }

    #[test]
    fn rearrange_is_selected_on_open() {
        let root = LayoutNode::split(
            SplitDirection::Down,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            picker.items[picker.selected],
            PickerItem::Action(PickerAction::Rearrange)
        );
    }

    fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column,
            row,
            modifiers: crossterm::event::KeyModifiers::NONE,
        }
    }

    #[test]
    fn layouts_tab_opens_on_the_current_preset() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));

        picker.switch_to_tab(PickerTab::Layouts);

        assert_eq!(
            picker.items[picker.selected],
            PickerItem::Preset(LayoutPreset::Columns)
        );
        assert_eq!(picker.item_hint(picker.selected), Some("current"));
        assert_eq!(picker.preview_label(&HashMap::new()), "Current layout");
    }

    #[test]
    fn tabs_keep_the_list_and_preview_geometry_stable() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        let backend = ratatui::backend::TestBackend::new(64, 18);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| picker.render(frame))
            .unwrap_or_else(|error| panic!("{error}"));
        let action_row = picker
            .row_areas
            .iter()
            .find(|(index, _)| *index == picker.selected)
            .map(|(_, area)| *area)
            .unwrap_or_default();
        let action_preview = picker.pane_areas.clone();

        picker.switch_to_tab(PickerTab::Layouts);
        terminal
            .draw(|frame| picker.render(frame))
            .unwrap_or_else(|error| panic!("{error}"));
        let layout_row = picker
            .row_areas
            .iter()
            .find(|(index, _)| *index == picker.selected)
            .map(|(_, area)| *area)
            .unwrap_or_default();

        assert_eq!(action_row.y, layout_row.y);
        assert_eq!(action_preview, picker.pane_areas);
    }

    #[test]
    fn clicking_a_tab_switches_the_visible_list() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        let backend = ratatui::backend::TestBackend::new(64, 18);
        let mut terminal = Terminal::new(backend).unwrap_or_else(|error| panic!("{error}"));
        terminal
            .draw(|frame| picker.render(frame))
            .unwrap_or_else(|error| panic!("{error}"));
        let layouts_area = picker
            .tab_areas
            .iter()
            .find(|(tab, _)| *tab == PickerTab::Layouts)
            .map(|(_, area)| *area)
            .unwrap_or_default();

        picker.handle_mouse(mouse(
            MouseEventKind::Down(MouseButton::Left),
            layouts_area.x,
            layouts_area.y,
        ));

        assert_eq!(picker.active_tab, PickerTab::Layouts);
        assert!(matches!(
            picker.items[picker.selected],
            PickerItem::Preset(_)
        ));
    }

    #[test]
    fn corners_choose_the_nearest_edge() {
        let area = Rect::new(10, 10, 20, 10);
        assert!(matches!(drop_zone(area, 10, 12), DropZone::Left));
        assert!(matches!(drop_zone(area, 29, 18), DropZone::Right));
        assert!(matches!(drop_zone(area, 20, 15), DropZone::Center));
    }

    #[test]
    fn pane_numbers_stay_with_ids_after_reordering() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        let reordered = vec!["p2".into(), "p1".into()];

        picker.ensure_pane_numbers(&reordered);

        assert_eq!(picker.pane_numbers.get("p1"), Some(&1));
        assert_eq!(picker.pane_numbers.get("p2"), Some(&2));
    }

    #[test]
    fn hovering_a_pane_exposes_its_drag_affordance() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        picker.pane_areas = vec![
            ("p1".into(), Rect::new(0, 0, 10, 10)),
            ("p2".into(), Rect::new(11, 0, 10, 10)),
        ];

        picker.handle_mouse(mouse(MouseEventKind::Moved, 15, 5));

        assert_eq!(picker.hovered_pane.as_deref(), Some("p2"));
        let numbers = HashMap::from([("p1".into(), 1), ("p2".into(), 2)]);
        assert_eq!(picker.preview_label(&numbers), "Drag pane 2");
    }

    #[test]
    fn escape_unwinds_keyboard_rearranging_one_step_at_a_time() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        picker.focus_preview();
        picker.handle_key(KeyCode::Char(' '));
        assert!(picker.drag_source.is_some());

        picker.handle_key(KeyCode::Esc);
        assert!(picker.preview_focused);
        assert!(picker.drag_source.is_none());

        picker.handle_key(KeyCode::Esc);
        assert!(!picker.preview_focused);
    }

    #[test]
    fn dragging_to_an_edge_builds_the_expected_layout() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p2")).unwrap_or_else(|error| panic!("{error}"));
        picker.selected = picker
            .items
            .iter()
            .position(|item| *item == PickerItem::Action(PickerAction::Rearrange))
            .unwrap_or_default();
        picker.pane_areas = vec![
            ("p1".into(), Rect::new(0, 0, 10, 10)),
            ("p2".into(), Rect::new(11, 0, 10, 10)),
        ];

        picker.handle_mouse(mouse(MouseEventKind::Down(MouseButton::Left), 15, 5));
        picker.handle_mouse(mouse(MouseEventKind::Drag(MouseButton::Left), 0, 5));
        let request = picker.handle_mouse(mouse(MouseEventKind::Up(MouseButton::Left), 0, 5));
        let PickerRequest::ApplyLayout(target) = request else {
            panic!("drag did not produce a layout");
        };

        assert_eq!(target.pane_ids().unwrap_or_default(), vec!["p2", "p1"]);
        assert!(matches!(
            target,
            LayoutNode::Split {
                direction: SplitDirection::Right,
                ..
            }
        ));
    }

    #[test]
    fn unavailable_undo_is_skipped() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));
        let rotate = picker
            .items
            .iter()
            .position(|item| *item == PickerItem::Action(PickerAction::Rotate))
            .unwrap_or_default();
        picker.select(rotate);

        picker.move_selection(1);

        assert_eq!(
            picker.items[picker.selected],
            PickerItem::Action(PickerAction::Rearrange)
        );
    }

    #[test]
    fn switching_tabs_preserves_each_selection() {
        let root = LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::pane("p2".into()),
        );
        let mut picker = Picker::new(layout(root, "p1")).unwrap_or_else(|error| panic!("{error}"));

        picker.switch_tab();
        picker.move_selection(1);
        let layout_selection = picker.selected;
        picker.switch_tab();
        picker.move_selection(1);
        let action_selection = picker.selected;
        picker.switch_tab();
        assert_eq!(picker.selected, layout_selection);
        picker.switch_tab();
        assert_eq!(picker.selected, action_selection);
    }
}
