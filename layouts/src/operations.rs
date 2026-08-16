use std::{collections::HashSet, fmt};

use crate::{
    error::{AppError, AppResult},
    herdr::HerdrClient,
    layout::{LayoutDescription, LayoutNode, SplitDirection},
    state::{self, UndoRecord},
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LayoutPreset {
    Columns,
    Rows,
    Grid,
    Left,
    Right,
    Top,
    Bottom,
}

impl LayoutPreset {
    pub fn available(pane_count: usize) -> Vec<Self> {
        [
            Self::Columns,
            Self::Rows,
            Self::Grid,
            Self::Left,
            Self::Right,
            Self::Top,
            Self::Bottom,
        ]
        .into_iter()
        .filter(|preset| *preset != Self::Grid || pane_count == 4)
        .collect()
    }

    pub fn is_equal(self) -> bool {
        matches!(self, Self::Columns | Self::Rows | Self::Grid)
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            "columns" => Some(Self::Columns),
            "rows" => Some(Self::Rows),
            "grid" => Some(Self::Grid),
            "left" => Some(Self::Left),
            "right" => Some(Self::Right),
            "top" => Some(Self::Top),
            "bottom" => Some(Self::Bottom),
            _ => None,
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::Columns => "Equal columns",
            Self::Rows => "Equal rows",
            Self::Grid => "2 × 2 grid",
            Self::Left => "Focused pane on the left",
            Self::Right => "Focused pane on the right",
            Self::Top => "Focused pane on top",
            Self::Bottom => "Focused pane on the bottom",
        }
    }
}

impl fmt::Display for LayoutPreset {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::Columns => "Columns",
            Self::Rows => "Rows",
            Self::Grid => "Grid",
            Self::Left => "Left",
            Self::Right => "Right",
            Self::Top => "Top",
            Self::Bottom => "Bottom",
        };
        formatter.write_str(name)
    }
}

pub fn target_for_preset(
    layout: &LayoutDescription,
    preset: LayoutPreset,
) -> AppResult<LayoutNode> {
    let pane_ids = layout.root.pane_ids()?;
    if !(2..=4).contains(&pane_ids.len()) {
        return Err(AppError::Message(
            "Arrange supports two to four panes".into(),
        ));
    }

    if preset == LayoutPreset::Grid && pane_ids.len() != 4 {
        return Err(AppError::Message("Grid requires four panes".into()));
    }

    let focused = layout.focused_pane_id.clone();
    let secondary: Vec<String> = pane_ids
        .iter()
        .filter(|pane_id| **pane_id != focused)
        .cloned()
        .collect();
    let target = match preset {
        LayoutPreset::Columns => equal_sequence(&pane_ids, SplitDirection::Right)?,
        LayoutPreset::Rows => equal_sequence(&pane_ids, SplitDirection::Down)?,
        LayoutPreset::Grid => LayoutNode::split(
            SplitDirection::Down,
            0.5,
            LayoutNode::split(
                SplitDirection::Right,
                0.5,
                LayoutNode::pane(pane_ids[0].clone()),
                LayoutNode::pane(pane_ids[1].clone()),
            ),
            LayoutNode::split(
                SplitDirection::Right,
                0.5,
                LayoutNode::pane(pane_ids[2].clone()),
                LayoutNode::pane(pane_ids[3].clone()),
            ),
        ),
        LayoutPreset::Left => LayoutNode::split(
            SplitDirection::Right,
            2.0 / 3.0,
            LayoutNode::pane(focused),
            equal_sequence(&secondary, SplitDirection::Down)?,
        ),
        LayoutPreset::Right => LayoutNode::split(
            SplitDirection::Right,
            1.0 / 3.0,
            equal_sequence(&secondary, SplitDirection::Down)?,
            LayoutNode::pane(focused),
        ),
        LayoutPreset::Top => LayoutNode::split(
            SplitDirection::Down,
            2.0 / 3.0,
            LayoutNode::pane(focused),
            equal_sequence(&secondary, SplitDirection::Right)?,
        ),
        LayoutPreset::Bottom => LayoutNode::split(
            SplitDirection::Down,
            1.0 / 3.0,
            equal_sequence(&secondary, SplitDirection::Right)?,
            LayoutNode::pane(focused),
        ),
    };
    Ok(target)
}

pub fn apply_preset(client: &HerdrClient, pane_id: &str, preset: LayoutPreset) -> AppResult<()> {
    let before = client.export_layout(pane_id)?;
    validate_mutation(&before)?;
    let target = target_for_preset(&before, preset)?;
    apply_tree_with_undo(client, before, target)
}

pub fn apply_layout(client: &HerdrClient, pane_id: &str, target: LayoutNode) -> AppResult<()> {
    let before = client.export_layout(pane_id)?;
    validate_mutation(&before)?;
    validate_same_panes(&before.root, &target)?;
    apply_tree_with_undo(client, before, target)
}

pub fn expand(client: &HerdrClient, pane_id: &str) -> AppResult<()> {
    apply_ratio(client, pane_id, expanded_ratio)
}

pub fn target_for_expand(layout: &LayoutDescription) -> AppResult<LayoutNode> {
    target_for_ratio(layout, expanded_ratio)
}

pub fn balance(client: &HerdrClient, pane_id: &str) -> AppResult<()> {
    apply_ratio(client, pane_id, |_, _| 0.5)
}

pub fn target_for_balance(layout: &LayoutDescription) -> AppResult<LayoutNode> {
    target_for_ratio(layout, |_, _| 0.5)
}

fn expanded_ratio(ratio: f64, focused_is_second: bool) -> f64 {
    if focused_is_second {
        ratio.min(1.0 / 3.0)
    } else {
        ratio.max(2.0 / 3.0)
    }
}

pub fn rotate(client: &HerdrClient, pane_id: &str) -> AppResult<()> {
    let before = client.export_layout(pane_id)?;
    validate_mutation(&before)?;
    let target = target_for_rotate(&before)?;
    apply_tree_with_undo(client, before, target)
}

pub fn target_for_rotate(layout: &LayoutDescription) -> AppResult<LayoutNode> {
    let path = layout
        .root
        .closest_split_path(&layout.focused_pane_id)
        .ok_or_else(|| AppError::Message("The focused pane has no split to rotate".into()))?;
    let split = layout
        .root
        .node_at_path(&path)
        .cloned()
        .ok_or_else(|| AppError::Message("The focused split no longer exists".into()))?;
    let replacement = match split {
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => LayoutNode::Split {
            direction: direction.rotated(),
            ratio,
            first,
            second,
        },
        LayoutNode::Pane { .. } => {
            return Err(AppError::Message(
                "The focused pane has no split to rotate".into(),
            ));
        }
    };
    layout.root.with_node_at_path(&path, replacement)
}

pub fn undo_target(layout: &LayoutDescription) -> AppResult<Option<LayoutNode>> {
    let Some(record) = state::get(&layout.tab_id)? else {
        return Ok(None);
    };
    if layout.root.normalized()? != record.after {
        return Ok(None);
    }
    Ok(Some(record.before))
}

pub fn undo(client: &HerdrClient, pane_id: &str) -> AppResult<bool> {
    let current = client.export_layout(pane_id)?;
    validate_mutation(&current)?;
    let Some(record) = state::get(&current.tab_id)? else {
        return Ok(false);
    };

    if current.root.normalized()? != record.after {
        state::remove(&current.tab_id)?;
        return Ok(false);
    }

    if try_rearrange(client, &current.tab_id, &record.before)?.is_none() {
        rebuild_tree(client, &current, &record.before)?;
    }
    state::remove(&current.tab_id)?;
    Ok(true)
}

fn apply_ratio<F>(client: &HerdrClient, pane_id: &str, choose_ratio: F) -> AppResult<()>
where
    F: FnOnce(f64, bool) -> f64,
{
    let before = client.export_layout(pane_id)?;
    validate_mutation(&before)?;
    let change = ratio_change(&before, choose_ratio)?;

    if (change.before_ratio - change.after_ratio).abs() < 0.0001 {
        return Ok(());
    }

    client.set_split_ratio(&before.tab_id, &change.split_path, change.after_ratio)?;
    let after = client.export_layout(pane_id)?;
    if let Err(error) = save_undo(&before, &after) {
        client.set_split_ratio(&before.tab_id, &change.split_path, change.before_ratio)?;
        return Err(error);
    }
    Ok(())
}

fn target_for_ratio<F>(layout: &LayoutDescription, choose_ratio: F) -> AppResult<LayoutNode>
where
    F: FnOnce(f64, bool) -> f64,
{
    let change = ratio_change(layout, choose_ratio)?;
    let split = layout
        .root
        .node_at_path(&change.split_path)
        .cloned()
        .ok_or_else(|| AppError::Message("The focused split no longer exists".into()))?;
    let LayoutNode::Split {
        direction,
        first,
        second,
        ..
    } = split
    else {
        return Err(AppError::Message(
            "The focused split no longer exists".into(),
        ));
    };
    layout.root.with_node_at_path(
        &change.split_path,
        LayoutNode::Split {
            direction,
            ratio: change.after_ratio,
            first,
            second,
        },
    )
}

fn ratio_change<F>(layout: &LayoutDescription, choose_ratio: F) -> AppResult<RatioChange>
where
    F: FnOnce(f64, bool) -> f64,
{
    let pane_path = layout
        .root
        .pane_path(&layout.focused_pane_id)
        .ok_or_else(|| AppError::Message("The focused pane is not in the current layout".into()))?;
    let focused_is_second = *pane_path
        .last()
        .ok_or_else(|| AppError::Message("The focused pane has no split to resize".into()))?;
    let split_path = pane_path[..pane_path.len() - 1].to_vec();
    let before_ratio = match layout.root.node_at_path(&split_path) {
        Some(LayoutNode::Split { ratio, .. }) => *ratio,
        _ => {
            return Err(AppError::Message(
                "The focused split no longer exists".into(),
            ));
        }
    };
    let after_ratio = choose_ratio(before_ratio, focused_is_second);
    Ok(RatioChange {
        split_path,
        before_ratio,
        after_ratio,
    })
}

struct RatioChange {
    split_path: Vec<bool>,
    before_ratio: f64,
    after_ratio: f64,
}

#[derive(Debug, PartialEq)]
enum FastMutation {
    Swap { source: String, target: String },
}

fn apply_tree_with_undo(
    client: &HerdrClient,
    before: LayoutDescription,
    target: LayoutNode,
) -> AppResult<()> {
    if before.root.normalized()? == target.normalized()? {
        return Ok(());
    }

    if let Some(mutation) = detect_fast_mutation(&before.root, &target)? {
        return apply_fast_mutation(client, &before, &target, mutation);
    }

    if let Some(after) = try_rearrange(client, &before.tab_id, &target)? {
        if after.root.normalized()? != target.normalized()? {
            client.rearrange_layout(&before.tab_id, &before.root)?;
            return Err(AppError::Message(
                "Herdr built a different layout than requested".into(),
            ));
        }
        if let Err(error) = save_undo(&before, &after) {
            client.rearrange_layout(&before.tab_id, &before.root)?;
            return Err(error);
        }
        return Ok(());
    }

    let after = rebuild_tree(client, &before, &target)?;
    if let Err(error) = save_undo(&before, &after) {
        rebuild_tree(client, &after, &before.root)?;
        return Err(error);
    }
    Ok(())
}

fn try_rearrange(
    client: &HerdrClient,
    tab_id: &str,
    target: &LayoutNode,
) -> AppResult<Option<LayoutDescription>> {
    match client.rearrange_layout(tab_id, target) {
        Ok(layout) => Ok(Some(layout)),
        Err(AppError::Herdr { code, message })
            if (code == "invalid_request" && message.contains("layout.rearrange"))
                || code == "method_not_found" =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn detect_fast_mutation(
    current: &LayoutNode,
    target: &LayoutNode,
) -> AppResult<Option<FastMutation>> {
    let pane_ids = current.pane_ids()?;
    let normalized_target = target.normalized()?;

    for (index, source) in pane_ids.iter().enumerate() {
        for target in pane_ids.iter().skip(index + 1) {
            if current.swap_panes(source, target)?.normalized()? == normalized_target {
                return Ok(Some(FastMutation::Swap {
                    source: source.clone(),
                    target: target.clone(),
                }));
            }
        }
    }

    Ok(None)
}

fn apply_fast_mutation(
    client: &HerdrClient,
    before: &LayoutDescription,
    target: &LayoutNode,
    mutation: FastMutation,
) -> AppResult<()> {
    let result = match &mutation {
        FastMutation::Swap { source, target } => client.swap_panes(source, target),
    };
    if let Err(error) = result {
        if let Ok(partial) = client.export_layout(&before.focused_pane_id)
            && partial.root.normalized()? != before.root.normalized()?
        {
            rebuild_tree(client, &partial, &before.root)?;
        }
        return Err(error);
    }

    let after = client.export_layout(&before.focused_pane_id)?;
    if after.root.normalized()? != target.normalized()? {
        rebuild_tree(client, &after, &before.root)?;
        return Err(AppError::Message(
            "Herdr built a different layout than requested".into(),
        ));
    }
    if let Err(error) = save_undo(before, &after) {
        rebuild_tree(client, &after, &before.root)?;
        return Err(error);
    }
    Ok(())
}

fn save_undo(before: &LayoutDescription, after: &LayoutDescription) -> AppResult<()> {
    state::save(UndoRecord {
        workspace_id: before.workspace_id.clone(),
        tab_id: before.tab_id.clone(),
        focused_pane_id: before.focused_pane_id.clone(),
        before: before.root.clone(),
        after: after.root.normalized()?,
    })
}

fn validate_mutation(layout: &LayoutDescription) -> AppResult<()> {
    if layout.zoomed {
        return Err(AppError::Message(
            "Unzoom this tab to change its layout".into(),
        ));
    }

    if !(2..=4).contains(&layout.root.pane_count()?) {
        return Err(AppError::Message(
            "Arrange works with two to four panes".into(),
        ));
    }

    Ok(())
}

fn equal_sequence(pane_ids: &[String], direction: SplitDirection) -> AppResult<LayoutNode> {
    let Some((first, rest)) = pane_ids.split_first() else {
        return Err(AppError::Message("A layout needs at least one pane".into()));
    };

    if rest.is_empty() {
        return Ok(LayoutNode::pane(first.clone()));
    }

    let ratio = 1.0 / pane_ids.len() as f64;
    let node = LayoutNode::split(
        direction,
        ratio,
        LayoutNode::pane(first.clone()),
        equal_sequence(rest, direction)?,
    );
    Ok(node)
}

fn rebuild_tree(
    client: &HerdrClient,
    before: &LayoutDescription,
    target: &LayoutNode,
) -> AppResult<LayoutDescription> {
    validate_same_panes(&before.root, target)?;
    let (parking_tab, parking_root) = client.create_parking_tab(&before.workspace_id)?;
    let result = rebuild_with_parking(
        client,
        &before.tab_id,
        &parking_tab,
        &parking_root,
        target,
        &before.focused_pane_id,
    );

    match result {
        Ok(after) => {
            close_parking_tab(client, &parking_tab);
            Ok(after)
        }
        Err(error) => {
            let rollback = rebuild_with_parking(
                client,
                &before.tab_id,
                &parking_tab,
                &parking_root,
                &before.root,
                &before.focused_pane_id,
            );

            match rollback {
                Ok(_) => {
                    close_parking_tab(client, &parking_tab);
                    Err(error)
                }
                Err(rollback_error) => {
                    client.notify(
                        "Arrange needs attention",
                        &format!("A pane remains safe in temporary tab {parking_tab}."),
                    );
                    Err(AppError::Message(format!(
                        "{error}; restoring the original layout also failed: {rollback_error}"
                    )))
                }
            }
        }
    }
}

fn close_parking_tab(client: &HerdrClient, parking_tab: &str) {
    if client.close_tab(parking_tab).is_err() {
        client.notify(
            "Arrange",
            &format!("Close temporary tab {parking_tab} when convenient."),
        );
    }
}

fn rebuild_with_parking(
    client: &HerdrClient,
    source_tab: &str,
    parking_tab: &str,
    parking_root: &str,
    target: &LayoutNode,
    focused_pane_id: &str,
) -> AppResult<LayoutDescription> {
    let target_pane_ids = target.pane_ids()?;
    let target_anchor = target.first_pane_id()?;
    let anchor_layout = client.export_layout(&target_anchor)?;

    if anchor_layout.tab_id != source_tab {
        let source_pane = target_pane_ids
            .iter()
            .map(|pane_id| {
                client
                    .export_layout(pane_id)
                    .map(|layout| (pane_id, layout))
            })
            .collect::<AppResult<Vec<_>>>()?
            .into_iter()
            .find(|(_, layout)| layout.tab_id == source_tab)
            .map(|(pane_id, _)| pane_id.clone())
            .ok_or_else(|| AppError::Message("The source tab lost every user pane".into()))?;
        client.move_pane(
            &target_anchor,
            source_tab,
            &source_pane,
            SplitDirection::Right,
            0.5,
            target_anchor == focused_pane_id,
        )?;
    }

    let source_layout = if anchor_layout.tab_id == source_tab {
        anchor_layout
    } else {
        client.export_layout(&target_anchor)?
    };
    source_layout
        .root
        .pane_ids()?
        .into_iter()
        .filter(|pane_id| pane_id != &target_anchor)
        .try_for_each(|pane_id| {
            client
                .move_pane(
                    &pane_id,
                    parking_tab,
                    parking_root,
                    SplitDirection::Right,
                    0.5,
                    false,
                )
                .map(|_| ())
        })?;

    insertion_steps(target)?.into_iter().try_for_each(|step| {
        client
            .move_pane(
                &step.pane_id,
                source_tab,
                &step.target_pane_id,
                step.direction,
                step.ratio,
                step.pane_id == focused_pane_id,
            )
            .map(|_| ())
    })?;

    let after = client.export_layout(&target_anchor)?;
    if after.root.normalized()? != target.normalized()? {
        return Err(AppError::Message(
            "Herdr built a different layout than requested".into(),
        ));
    }

    Ok(after)
}

fn validate_same_panes(current: &LayoutNode, target: &LayoutNode) -> AppResult<()> {
    let current_ids: HashSet<String> = current.pane_ids()?.into_iter().collect();
    let target_ids: HashSet<String> = target.pane_ids()?.into_iter().collect();

    if current_ids != target_ids {
        return Err(AppError::Message(
            "The requested layout does not contain the same panes".into(),
        ));
    }

    Ok(())
}

fn insertion_steps(node: &LayoutNode) -> AppResult<Vec<InsertionStep>> {
    match node {
        LayoutNode::Pane { .. } => Ok(Vec::new()),
        LayoutNode::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let first_pane_id = first.first_pane_id()?;
            let second_pane_id = second.first_pane_id()?;
            let step = InsertionStep {
                pane_id: second_pane_id,
                target_pane_id: first_pane_id,
                direction: *direction,
                ratio: *ratio,
            };
            let steps = std::iter::once(step)
                .chain(insertion_steps(first)?)
                .chain(insertion_steps(second)?)
                .collect();
            Ok(steps)
        }
    }
}

struct InsertionStep {
    pane_id: String,
    target_pane_id: String,
    direction: SplitDirection,
    ratio: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::LayoutDescription;

    fn layout(pane_count: usize, focused: usize) -> LayoutDescription {
        let pane_ids: Vec<String> = (1..=pane_count).map(|index| format!("p{index}")).collect();
        LayoutDescription {
            workspace_id: "w1".into(),
            tab_id: "t1".into(),
            zoomed: false,
            focused_pane_id: pane_ids[focused].clone(),
            root: equal_sequence(&pane_ids, SplitDirection::Right)
                .unwrap_or_else(|error| panic!("{error}")),
        }
    }

    #[test]
    fn main_left_keeps_the_focused_pane_first() {
        let target = target_for_preset(&layout(4, 2), LayoutPreset::Left)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(target.first_pane_id().unwrap_or_default(), "p3");
        assert_eq!(
            target.pane_ids().unwrap_or_default(),
            vec!["p3", "p1", "p2", "p4"]
        );
    }

    #[test]
    fn grid_uses_row_reading_order() {
        let target = target_for_preset(&layout(4, 0), LayoutPreset::Grid)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            target.pane_ids().unwrap_or_default(),
            vec!["p1", "p2", "p3", "p4"]
        );
        assert_eq!(insertion_steps(&target).unwrap_or_default().len(), 3);
    }

    #[test]
    fn detects_a_swap_as_one_native_mutation() {
        let current = layout(4, 0).root;
        let target = current
            .swap_panes("p1", "p4")
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            detect_fast_mutation(&current, &target).unwrap_or_default(),
            Some(FastMutation::Swap {
                source: "p1".into(),
                target: "p4".into(),
            })
        );
    }

    #[test]
    fn grid_is_only_available_for_four_panes() {
        assert!(!LayoutPreset::available(3).contains(&LayoutPreset::Grid));
        assert!(LayoutPreset::available(4).contains(&LayoutPreset::Grid));
    }
}
