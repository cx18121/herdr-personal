use serde::{Deserialize, Serialize};

use crate::error::{AppError, AppResult};

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDirection {
    Right,
    Down,
}

impl SplitDirection {
    pub fn rotated(self) -> Self {
        match self {
            Self::Right => Self::Down,
            Self::Down => Self::Right,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Right => "right",
            Self::Down => "down",
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum LayoutNode {
    Pane {
        pane_id: Option<String>,
        cwd: Option<String>,
        label: Option<String>,
        command: Option<Vec<String>>,
        #[serde(default)]
        env: std::collections::HashMap<String, String>,
    },
    Split {
        direction: SplitDirection,
        ratio: f64,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    pub fn pane(pane_id: String) -> Self {
        Self::Pane {
            pane_id: Some(pane_id),
            cwd: None,
            label: None,
            command: None,
            env: std::collections::HashMap::new(),
        }
    }

    pub fn split(
        direction: SplitDirection,
        ratio: f64,
        first: LayoutNode,
        second: LayoutNode,
    ) -> Self {
        Self::Split {
            direction,
            ratio,
            first: Box::new(first),
            second: Box::new(second),
        }
    }

    pub fn pane_ids(&self) -> AppResult<Vec<String>> {
        match self {
            Self::Pane { pane_id, .. } => {
                pane_id.clone().map(|pane_id| vec![pane_id]).ok_or_else(|| {
                    AppError::Message("The layout contains a pane without an ID".into())
                })
            }
            Self::Split { first, second, .. } => {
                let pane_ids = first
                    .pane_ids()?
                    .into_iter()
                    .chain(second.pane_ids()?)
                    .collect();
                Ok(pane_ids)
            }
        }
    }

    pub fn pane_count(&self) -> AppResult<usize> {
        self.pane_ids().map(|pane_ids| pane_ids.len())
    }

    pub fn first_pane_id(&self) -> AppResult<String> {
        match self {
            Self::Pane { pane_id, .. } => pane_id.clone().ok_or_else(|| {
                AppError::Message("The layout contains a pane without an ID".into())
            }),
            Self::Split { first, .. } => first.first_pane_id(),
        }
    }

    pub fn pane_path(&self, target: &str) -> Option<Vec<bool>> {
        fn find(node: &LayoutNode, target: &str, path: Vec<bool>) -> Option<Vec<bool>> {
            match node {
                LayoutNode::Pane { pane_id, .. } => pane_id
                    .as_deref()
                    .filter(|pane_id| *pane_id == target)
                    .map(|_| path),
                LayoutNode::Split { first, second, .. } => {
                    let mut first_path = path.clone();
                    first_path.push(false);
                    let mut second_path = path;
                    second_path.push(true);
                    find(first, target, first_path).or_else(|| find(second, target, second_path))
                }
            }
        }

        find(self, target, Vec::new())
    }

    pub fn closest_split_path(&self, target: &str) -> Option<Vec<bool>> {
        self.pane_path(target).and_then(|path| {
            if path.is_empty() {
                None
            } else {
                Some(path[..path.len() - 1].to_vec())
            }
        })
    }

    pub fn node_at_path(&self, path: &[bool]) -> Option<&LayoutNode> {
        path.iter().try_fold(self, |node, second| match node {
            Self::Split {
                first,
                second: right,
                ..
            } => {
                if *second {
                    Some(right.as_ref())
                } else {
                    Some(first.as_ref())
                }
            }
            Self::Pane { .. } => None,
        })
    }

    pub fn with_node_at_path(
        &self,
        path: &[bool],
        replacement: LayoutNode,
    ) -> AppResult<LayoutNode> {
        let Some((head, tail)) = path.split_first() else {
            return Ok(replacement);
        };

        match self {
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (next_first, next_second) = if *head {
                    (
                        first.as_ref().clone(),
                        second.with_node_at_path(tail, replacement)?,
                    )
                } else {
                    (
                        first.with_node_at_path(tail, replacement)?,
                        second.as_ref().clone(),
                    )
                };
                let node = Self::split(*direction, *ratio, next_first, next_second);
                Ok(node)
            }
            Self::Pane { .. } => Err(AppError::Message(
                "The split path is no longer valid".into(),
            )),
        }
    }

    pub fn swap_panes(&self, first_id: &str, second_id: &str) -> AppResult<Self> {
        if first_id == second_id
            || self.pane_path(first_id).is_none()
            || self.pane_path(second_id).is_none()
        {
            return Err(AppError::Message(
                "Choose two different panes to swap".into(),
            ));
        }

        self.map_pane_ids(&|pane_id| {
            if pane_id == first_id {
                second_id.to_string()
            } else if pane_id == second_id {
                first_id.to_string()
            } else {
                pane_id.to_string()
            }
        })
    }

    pub fn dock_pane(
        &self,
        pane_id: &str,
        target_id: &str,
        direction: SplitDirection,
        before_target: bool,
    ) -> AppResult<Self> {
        if pane_id == target_id
            || self.pane_path(pane_id).is_none()
            || self.pane_path(target_id).is_none()
        {
            return Err(AppError::Message(
                "Choose a different pane as the drop target".into(),
            ));
        }

        let remaining = self
            .without_pane(pane_id)?
            .ok_or_else(|| AppError::Message("A layout needs at least one pane".into()))?;
        let target_path = remaining
            .pane_path(target_id)
            .ok_or_else(|| AppError::Message("The drop target is no longer available".into()))?;
        let target = remaining
            .node_at_path(&target_path)
            .cloned()
            .ok_or_else(|| AppError::Message("The drop target is no longer available".into()))?;
        let moved = Self::pane(pane_id.to_string());
        let replacement = if before_target {
            Self::split(direction, 0.5, moved, target)
        } else {
            Self::split(direction, 0.5, target, moved)
        };
        remaining.with_node_at_path(&target_path, replacement)
    }

    fn without_pane(&self, target_id: &str) -> AppResult<Option<Self>> {
        match self {
            Self::Pane { pane_id, .. } => {
                let pane_id = pane_id.as_deref().ok_or_else(|| {
                    AppError::Message("The layout contains a pane without an ID".into())
                })?;
                Ok((pane_id != target_id).then(|| self.clone()))
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let first = first.without_pane(target_id)?;
                let second = second.without_pane(target_id)?;
                match (first, second) {
                    (Some(first), Some(second)) => {
                        Ok(Some(Self::split(*direction, *ratio, first, second)))
                    }
                    (Some(node), None) | (None, Some(node)) => Ok(Some(node)),
                    (None, None) => Ok(None),
                }
            }
        }
    }

    fn map_pane_ids(&self, map: &impl Fn(&str) -> String) -> AppResult<Self> {
        match self {
            Self::Pane { pane_id, .. } => {
                let pane_id = pane_id.as_deref().ok_or_else(|| {
                    AppError::Message("The layout contains a pane without an ID".into())
                })?;
                Ok(Self::pane(map(pane_id)))
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => Ok(Self::split(
                *direction,
                *ratio,
                first.map_pane_ids(map)?,
                second.map_pane_ids(map)?,
            )),
        }
    }

    pub fn normalized(&self) -> AppResult<ComparableLayoutNode> {
        match self {
            Self::Pane { pane_id, .. } => pane_id
                .clone()
                .map(ComparableLayoutNode::Pane)
                .ok_or_else(|| {
                    AppError::Message("The layout contains a pane without an ID".into())
                }),
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let node = ComparableLayoutNode::Split {
                    direction: *direction,
                    ratio: (ratio * 10_000.0).round() as i64,
                    first: Box::new(first.normalized()?),
                    second: Box::new(second.normalized()?),
                };
                Ok(node)
            }
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ComparableLayoutNode {
    Pane(String),
    Split {
        direction: SplitDirection,
        ratio: i64,
        first: Box<ComparableLayoutNode>,
        second: Box<ComparableLayoutNode>,
    },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct LayoutDescription {
    pub workspace_id: String,
    pub tab_id: String,
    pub zoomed: bool,
    pub focused_pane_id: String,
    pub root: LayoutNode,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn three_panes() -> LayoutNode {
        LayoutNode::split(
            SplitDirection::Right,
            0.5,
            LayoutNode::pane("p1".into()),
            LayoutNode::split(
                SplitDirection::Down,
                0.5,
                LayoutNode::pane("p2".into()),
                LayoutNode::pane("p3".into()),
            ),
        )
    }

    #[test]
    fn swapping_panes_keeps_the_layout_shape() {
        let swapped = three_panes()
            .swap_panes("p1", "p3")
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            swapped.pane_ids().unwrap_or_default(),
            vec!["p3", "p2", "p1"]
        );
        assert!(matches!(
            swapped,
            LayoutNode::Split {
                direction: SplitDirection::Right,
                ..
            }
        ));
    }

    #[test]
    fn docking_a_pane_collapses_its_old_split() {
        let docked = three_panes()
            .dock_pane("p3", "p1", SplitDirection::Down, true)
            .unwrap_or_else(|error| panic!("{error}"));

        assert_eq!(
            docked.pane_ids().unwrap_or_default(),
            vec!["p3", "p1", "p2"]
        );
        let p3_path = docked.pane_path("p3").unwrap_or_default();
        let p1_path = docked.pane_path("p1").unwrap_or_default();
        assert_eq!(&p3_path[..p3_path.len() - 1], &p1_path[..p1_path.len() - 1]);
    }
}
