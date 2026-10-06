use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Axis {
    Horizontal,
    Vertical,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum View {
    Files,
    Git,
    Editor(u64),
    Terminal(u64),
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Pane {
        id: u64,
        tabs: Vec<View>,
        active: usize,
    },
    Split {
        id: u64,
        axis: Axis,
        ratio: f32,
        first: Box<Node>,
        second: Box<Node>,
    },
}
#[derive(Clone, Copy, Default, Debug, Serialize)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}
#[derive(Clone, Debug, Serialize)]
pub struct Placement {
    pub id: u64,
    pub rect: Rect,
}
#[derive(Clone, Debug, Serialize)]
pub struct Handle {
    pub id: u64,
    pub axis: Axis,
    pub rect: Rect,
    pub parent: Rect,
}
impl Node {
    pub fn pane(id: u64, view: View) -> Self {
        Self::Pane {
            id,
            tabs: vec![view],
            active: 0,
        }
    }
    pub fn default_layout(editor: u64, terminal: u64) -> Self {
        Self::Split {
            id: 4,
            axis: Axis::Horizontal,
            ratio: 0.2,
            first: Box::new(Self::Pane {
                id: 1,
                tabs: vec![View::Files, View::Git],
                active: 0,
            }),
            second: Box::new(Self::Split {
                id: 5,
                axis: Axis::Horizontal,
                ratio: 0.67,
                first: Box::new(Self::pane(2, View::Editor(editor))),
                second: Box::new(Self::pane(3, View::Terminal(terminal))),
            }),
        }
    }
    pub fn pane_mut(&mut self, target: u64) -> Option<(&mut Vec<View>, &mut usize)> {
        match self {
            Self::Pane { id, tabs, active } if *id == target => Some((tabs, active)),
            Self::Split { first, second, .. } => {
                first.pane_mut(target).or_else(|| second.pane_mut(target))
            }
            _ => None,
        }
    }
    pub fn view(&self, target: u64) -> Option<&View> {
        match self {
            Self::Pane { id, tabs, active } if *id == target => tabs.get(*active),
            Self::Split { first, second, .. } => first.view(target).or_else(|| second.view(target)),
            _ => None,
        }
    }
    pub fn panes(&self) -> Vec<u64> {
        match self {
            Self::Pane { id, .. } => vec![*id],
            Self::Split { first, second, .. } => [first.panes(), second.panes()].concat(),
        }
    }
    pub fn split(
        &mut self,
        target: u64,
        axis: Axis,
        new_pane: u64,
        new_split: u64,
        view: View,
    ) -> bool {
        match self {
            Self::Pane { id, .. } if *id == target => {
                let old = self.clone();
                *self = Self::Split {
                    id: new_split,
                    axis,
                    ratio: 0.5,
                    first: Box::new(old),
                    second: Box::new(Self::pane(new_pane, view)),
                };
                true
            }
            Self::Split { first, second, .. } => {
                first.split(target, axis, new_pane, new_split, view.clone())
                    || second.split(target, axis, new_pane, new_split, view)
            }
            _ => false,
        }
    }
    pub fn remove(&mut self, target: u64) -> bool {
        if let Self::Split { first, second, .. } = self {
            if matches!(first.as_ref(),Self::Pane{id,..} if *id==target) {
                *self = *second.clone();
                return true;
            }
            if matches!(second.as_ref(),Self::Pane{id,..} if *id==target) {
                *self = *first.clone();
                return true;
            }
            return first.remove(target) || second.remove(target);
        }
        false
    }
    pub fn resize(&mut self, target: u64, value: f32) -> bool {
        match self {
            Self::Split { id, ratio, .. } if *id == target => {
                if value.is_finite() {
                    *ratio = value.clamp(0.1, 0.9);
                }
                true
            }
            Self::Split { first, second, .. } => {
                first.resize(target, value) || second.resize(target, value)
            }
            _ => false,
        }
    }
    pub fn swap(&mut self, a: u64, b: u64) -> bool {
        let Some((tabs, active)) = self.pane_mut(a) else {
            return false;
        };
        let left = (tabs.clone(), *active);
        let Some((tabs, active)) = self.pane_mut(b) else {
            return false;
        };
        let right = (tabs.clone(), *active);
        *tabs = left.0;
        *active = left.1;
        let (tabs, active) = self.pane_mut(a).unwrap();
        *tabs = right.0;
        *active = right.1;
        true
    }
    pub fn arrange(&self, rect: Rect, gap: u16) -> (Vec<Placement>, Vec<Handle>) {
        let mut panes = vec![];
        let mut handles = vec![];
        self.walk(rect, gap, (0, 0), &mut panes, &mut handles);
        (panes, handles)
    }
    /// Minimum size of the complete split tree, including its handles.
    pub fn minimum_size(&self, gap: u16, minimum: (u16, u16)) -> (u16, u16) {
        match self {
            Self::Pane { .. } => minimum,
            Self::Split {
                axis,
                first,
                second,
                ..
            } => {
                let a = first.minimum_size(gap, minimum);
                let b = second.minimum_size(gap, minimum);
                match axis {
                    Axis::Horizontal => (a.0.saturating_add(b.0).saturating_add(gap), a.1.max(b.1)),
                    Axis::Vertical => (a.0.max(b.0), a.1.saturating_add(b.1).saturating_add(gap)),
                }
            }
        }
    }
    /// Constrain displayed ratios without changing the user's saved preferences.
    /// Callers must provide an area at least as large as `minimum_size`.
    pub fn arrange_constrained(
        &self,
        rect: Rect,
        gap: u16,
        minimum: (u16, u16),
    ) -> (Vec<Placement>, Vec<Handle>) {
        let mut panes = vec![];
        let mut handles = vec![];
        self.walk(rect, gap, minimum, &mut panes, &mut handles);
        (panes, handles)
    }
    fn walk(
        &self,
        r: Rect,
        gap: u16,
        minimum: (u16, u16),
        p: &mut Vec<Placement>,
        h: &mut Vec<Handle>,
    ) {
        match self {
            Self::Pane { id, .. } => p.push(Placement { id: *id, rect: r }),
            Self::Split {
                id,
                axis,
                ratio,
                first,
                second,
            } => {
                let length = if *axis == Axis::Horizontal {
                    r.width
                } else {
                    r.height
                };
                let gap = gap.min(length);
                let desired = ((length - gap) as f32 * ratio.clamp(0.1, 0.9)).round() as u16;
                let a_min = first.minimum_size(gap, minimum);
                let b_min = second.minimum_size(gap, minimum);
                let (low, reserved) = if *axis == Axis::Horizontal {
                    (a_min.0, b_min.0)
                } else {
                    (a_min.1, b_min.1)
                };
                let high = (length - gap).saturating_sub(reserved);
                let size = desired.clamp(low.min(high), high);
                let (a, b, handle) = if *axis == Axis::Horizontal {
                    (
                        Rect { width: size, ..r },
                        Rect {
                            x: r.x + size + gap,
                            width: length - size - gap,
                            ..r
                        },
                        Rect {
                            x: r.x + size,
                            width: gap,
                            ..r
                        },
                    )
                } else {
                    (
                        Rect { height: size, ..r },
                        Rect {
                            y: r.y + size + gap,
                            height: length - size - gap,
                            ..r
                        },
                        Rect {
                            y: r.y + size,
                            height: gap,
                            ..r
                        },
                    )
                };
                h.push(Handle {
                    id: *id,
                    axis: *axis,
                    rect: handle,
                    parent: r,
                });
                first.walk(a, gap, minimum, p, h);
                second.walk(b, gap, minimum, p, h);
            }
        }
    }
    pub fn validate(&self) -> bool {
        fn check(n: &Node, depth: usize, ids: &mut Vec<u64>) -> bool {
            if depth > 12 {
                return false;
            }
            let id = match n {
                Node::Pane { id, .. } | Node::Split { id, .. } => *id,
            };
            if ids.contains(&id) {
                return false;
            }
            ids.push(id);
            match n {
                Node::Pane { tabs, active, .. } => {
                    !tabs.is_empty() && tabs.len() <= 32 && *active < tabs.len()
                }
                Node::Split {
                    ratio,
                    first,
                    second,
                    ..
                } => {
                    ratio.is_finite()
                        && *ratio >= 0.1
                        && *ratio <= 0.9
                        && check(first, depth + 1, ids)
                        && check(second, depth + 1, ids)
                }
            }
        }
        check(self, 0, &mut vec![])
    }
}
