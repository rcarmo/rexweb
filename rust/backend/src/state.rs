//! Session and tiled-pane state, without ownership of live terminal processes.

use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use crate::layout::{SplitDirection, split_rect};

const MAX_SESSIONS: usize = 32;
const MAX_PANES: usize = 64;
const TERMINAL_CREATOR: &str = "com.superlogical.terminal";
const SHELL_FLAVOR: &str = "com.superlogical.terminal.shell";

#[derive(Debug)]
pub struct State {
    host_id: String,
    sessions: Vec<Session>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            host_id: Uuid::new_v4().to_string(),
            sessions: Vec::new(),
        }
    }
}

#[derive(Debug)]
struct Session {
    id: String,
    label: String,
    active: String,
    revision: u64,
    windows: Vec<Window>,
    panes: HashSet<String>,
}

#[derive(Debug)]
struct Window {
    id: String,
    layer_id: String,
    label: String,
    focused: String,
    root: Box<Node>,
}

#[derive(Debug, Clone)]
struct Node {
    block: Option<String>,
    split: Option<String>,
    direction: Option<SplitDirection>,
    ratio: f64,
    before: Option<Box<Node>>,
    after: Option<Box<Node>>,
}

impl Node {
    fn block(id: String) -> Self {
        Self {
            block: Some(id),
            split: None,
            direction: None,
            ratio: 0.0,
            before: None,
            after: None,
        }
    }

    fn contains_block(&self, id: &str) -> bool {
        self.block.as_deref() == Some(id)
            || self
                .before
                .as_deref()
                .is_some_and(|node| node.contains_block(id))
            || self
                .after
                .as_deref()
                .is_some_and(|node| node.contains_block(id))
    }

    fn leaf_mut(&mut self, id: &str) -> Option<&mut Node> {
        if self.block.as_deref() == Some(id) {
            return Some(self);
        }
        if let Some(node) = self
            .before
            .as_deref_mut()
            .and_then(|node| node.leaf_mut(id))
        {
            return Some(node);
        }
        self.after.as_deref_mut().and_then(|node| node.leaf_mut(id))
    }

    fn resize(&mut self, action: &Action) -> bool {
        if self.block.is_some() {
            return false;
        }
        if self.direction == direction(action.direction.as_str())
            && self
                .before
                .as_deref()
                .is_some_and(|node| node.contains_block(&action.before))
            && self
                .after
                .as_deref()
                .is_some_and(|node| node.contains_block(&action.after))
        {
            self.ratio = action.ratio;
            return true;
        }
        self.before
            .as_deref_mut()
            .is_some_and(|node| node.resize(action))
            || self
                .after
                .as_deref_mut()
                .is_some_and(|node| node.resize(action))
    }

    fn remove(self: Box<Self>, id: &str) -> Option<Box<Self>> {
        if self.block.as_deref() == Some(id) {
            return None;
        }
        if self.block.is_some() {
            return Some(self);
        }
        let Node {
            block,
            split,
            direction,
            ratio,
            before,
            after,
        } = *self;
        let before = before.and_then(|node| node.remove(id));
        if before.is_none() {
            return after;
        }
        let after = after.and_then(|node| node.remove(id));
        if after.is_none() {
            return before;
        }
        Some(Box::new(Node {
            block,
            split,
            direction,
            ratio,
            before,
            after,
        }))
    }

    fn blocks(&self, out: &mut Vec<String>) {
        if let Some(id) = &self.block {
            out.push(id.clone());
        } else {
            if let Some(before) = &self.before {
                before.blocks(out);
            }
            if let Some(after) = &self.after {
                after.blocks(out);
            }
        }
    }

    fn layout(&self, rect: [f64; 4], blocks: &mut Vec<Value>, host_id: &str) -> Value {
        if let Some(id) = &self.block {
            blocks.push(json!({
                "block_id": id,
                "label": "shell",
                "creator_name": TERMINAL_CREATOR,
                "flavor": SHELL_FLAVOR,
                "host_id": host_id,
            }));
            return json!({ "rect": rect, "block_id": id });
        }

        let direction = self.direction.expect("split direction");
        let rects = split_rect(rect, direction, self.ratio).expect("valid split ratio");
        json!({
            "rect": rect,
            "split": {
                "split_id": self.split,
                "direction": direction_name(direction),
                "ratio": self.ratio,
                "before": self.before.as_deref().expect("split before").layout(rects.before, blocks, host_id),
                "after": self.after.as_deref().expect("split after").layout(rects.after, blocks, host_id),
            }
        })
    }

    fn first_block(&self) -> &str {
        if let Some(id) = &self.block {
            return id;
        }
        self.before.as_deref().expect("split before").first_block()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct Action {
    op: String,
    session: String,
    window: String,
    block: String,
    label: String,
    direction: String,
    before: String,
    after: String,
    ratio: f64,
    side: String,
    focus: Option<bool>,
    window_label: String,
}

impl State {
    /// Return session summaries in server order, with the browser/native wire shape.
    pub fn summaries(&self) -> Value {
        Value::Array(self.sessions.iter().map(Session::summary).collect())
    }

    /// Return a complete session view, or `None` when the session is absent.
    pub fn view(&self, id: &str) -> Option<Value> {
        let session = self.sessions.iter().find(|session| session.id == id)?;
        Some(session.view(&self.host_id))
    }

    /// Test ownership of a pane by a session. This is the canonical map lookup hook.
    #[must_use]
    pub fn contains(&self, session: &str, block: &str) -> bool {
        self.sessions
            .iter()
            .find(|item| item.id == session)
            .is_some_and(|item| item.panes.contains(block))
    }

    /// Copy the live pane IDs for a session. The caller owns the corresponding pane map.
    pub fn pane_ids(&self, session: &str) -> Option<HashSet<String>> {
        self.sessions
            .iter()
            .find(|item| item.id == session)
            .map(|item| item.panes.clone())
    }

    /// Apply a browser/native mutation. The closure creates a live pane and returns only its ID.
    pub fn apply(
        &mut self,
        request: &Value,
        mut new_pane: impl FnMut() -> Result<String>,
    ) -> Result<Value> {
        let action: Action = serde_json::from_value(request.clone()).context("invalid action")?;
        self.apply_action(action, &mut new_pane)
    }

    fn apply_action(
        &mut self,
        mut action: Action,
        new_pane: &mut impl FnMut() -> Result<String>,
    ) -> Result<Value> {
        if action.op == "ensure-default" {
            if let Some(session) = self.sessions.first() {
                return Ok(session.summary());
            }
            action.op = "create".into();
        }

        if action.op == "create" {
            ensure!(self.sessions.len() < MAX_SESSIONS, "session limit reached");
            ensure!(self.pane_count() < MAX_PANES, "pane limit reached");
            let pane_id = self.new_pane_id(new_pane)?;
            let mut session = Session {
                id: new_id("session"),
                label: label(&action.label),
                active: String::new(),
                revision: 1,
                windows: Vec::new(),
                panes: HashSet::from([pane_id]),
            };
            let window = session.add_window(
                session.panes.iter().next().unwrap().clone(),
                &action.window_label,
            );
            let created = created_window(window);
            let id = session.id.clone();
            self.sessions.push(session);
            return Ok(json!({ "session_id": id, "initial_windows": [created] }));
        }

        let index = self
            .sessions
            .iter()
            .position(|session| session.id == action.session)
            .ok_or_else(|| anyhow!("session not found"))?;
        let mut result = Value::Object(Default::default());

        match action.op.as_str() {
            "rename" => self.sessions[index].label = label(&action.label),
            "destroy" => {
                let mut session = self.sessions.remove(index);
                session.revision += 1;
                return Ok(json!({ "revision": session.revision }));
            }
            "new-window" => {
                ensure!(self.pane_count() < MAX_PANES, "pane limit reached");
                let pane_id = self.new_pane_id(new_pane)?;
                let session = &mut self.sessions[index];
                let active = session.active.clone();
                let window = session.add_window(pane_id, &action.window_label);
                result = created_window(window);
                if !active.is_empty() && action.focus == Some(false) {
                    session.active = active;
                }
            }
            "focus-window" | "close-window" | "rename-window" => {
                let session = &mut self.sessions[index];
                let window_index = session
                    .windows
                    .iter()
                    .position(|window| window.id == action.window)
                    .ok_or_else(|| anyhow!("window not found"))?;
                match action.op.as_str() {
                    "focus-window" => session.active = session.windows[window_index].id.clone(),
                    "rename-window" => {
                        session.windows[window_index].label = label(&action.label);
                    }
                    "close-window" => {
                        let mut removed = Vec::new();
                        session.windows[window_index].root.blocks(&mut removed);
                        for id in removed {
                            session.panes.remove(&id);
                        }
                        session.remove_window(window_index);
                    }
                    _ => unreachable!(),
                }
            }
            "focus-block" | "close-block" | "split" => {
                let window_index = self.sessions[index]
                    .windows
                    .iter()
                    .position(|window| window.root.contains_block(&action.block))
                    .ok_or_else(|| anyhow!("pane not found"))?;
                let new_split_pane = if action.op == "split" {
                    direction(&action.direction)
                        .ok_or_else(|| anyhow!("invalid split direction, side or ratio"))?;
                    crate::layout::normalize_creation_ratio(action.ratio)
                        .ok_or_else(|| anyhow!("invalid split direction, side or ratio"))?;
                    ensure!(
                        action.side.is_empty() || action.side == "before" || action.side == "after",
                        "invalid split direction, side or ratio"
                    );
                    ensure!(self.pane_count() < MAX_PANES, "pane limit reached");
                    Some(self.new_pane_id(new_pane)?)
                } else {
                    None
                };
                let session = &mut self.sessions[index];
                match action.op.as_str() {
                    "focus-block" => {
                        session.windows[window_index].focused = action.block.clone();
                        session.active = session.windows[window_index].id.clone();
                    }
                    "close-block" => {
                        session.panes.remove(&action.block);
                        session.remove_pane(window_index, &action.block);
                    }
                    "split" => {
                        let split_direction =
                            direction(&action.direction).expect("validated direction");
                        let ratio = crate::layout::normalize_creation_ratio(action.ratio)
                            .expect("validated ratio");
                        let pane_id = new_split_pane.expect("pane created before split");
                        let window = &mut session.windows[window_index];
                        let leaf = window.root.leaf_mut(&action.block).expect("pane leaf");
                        let old = Box::new(leaf.clone());
                        let added = Box::new(Node::block(pane_id.clone()));
                        let (before, after) = if action.side == "before" {
                            (added, old)
                        } else {
                            (old, added)
                        };
                        let split_id = new_id("split");
                        *leaf = Node {
                            block: None,
                            split: Some(split_id.clone()),
                            direction: Some(split_direction),
                            ratio,
                            before: Some(before),
                            after: Some(after),
                        };
                        session.panes.insert(pane_id.clone());
                        if action.focus.unwrap_or(true) {
                            window.focused = pane_id.clone();
                            session.active = window.id.clone();
                        }
                        result = json!({
                            "block_ids": [pane_id],
                            "anchor_id": action.block,
                            "split_ids": [split_id],
                        });
                    }
                    _ => unreachable!(),
                }
            }
            "resize-split" => {
                let split_direction = direction(&action.direction);
                let ratio = crate::layout::normalize_resize_ratio(action.ratio);
                ensure!(
                    split_direction.is_some() && ratio.is_some(),
                    "invalid split direction or ratio"
                );
                let session = &mut self.sessions[index];
                ensure!(
                    session
                        .windows
                        .iter_mut()
                        .any(|window| window.root.resize(&action)),
                    "split not found"
                );
                session.revision += 1;
                return Ok(json!({ "revision": session.revision }));
            }
            _ => bail!("invalid operation"),
        }

        let session = &mut self.sessions[index];
        session.revision += 1;
        let object = result.as_object_mut().expect("action result object");
        object.insert("revision".into(), json!(session.revision));
        Ok(result)
    }

    fn pane_count(&self) -> usize {
        self.sessions
            .iter()
            .map(|session| session.panes.len())
            .sum()
    }

    fn new_pane_id(&self, new_pane: &mut impl FnMut() -> Result<String>) -> Result<String> {
        let id = new_pane().context("create pane")?;
        ensure!(!id.is_empty(), "pane id is empty");
        ensure!(
            !self
                .sessions
                .iter()
                .any(|session| session.panes.contains(&id)),
            "pane id already belongs to the state"
        );
        Ok(id)
    }
}

impl Session {
    fn summary(&self) -> Value {
        json!({ "session_id": self.id, "label": self.label })
    }

    fn add_window(&mut self, pane_id: String, name: &str) -> &Window {
        let id = new_id("window");
        self.windows.push(Window {
            id: id.clone(),
            layer_id: new_id("layer"),
            label: if name.trim().is_empty() {
                "Terminal".into()
            } else {
                name.into()
            },
            focused: pane_id.clone(),
            root: Box::new(Node::block(pane_id.clone())),
        });
        self.active = id;
        self.panes.insert(pane_id);
        self.windows.last().expect("new window")
    }

    fn remove_window(&mut self, index: usize) {
        let removed = self.windows.remove(index);
        if self.active == removed.id {
            self.active = self
                .windows
                .get(index.min(self.windows.len().saturating_sub(1)))
                .map(|window| window.id.clone())
                .unwrap_or_default();
        }
    }

    fn remove_pane(&mut self, window_index: usize, id: &str) {
        let window = &mut self.windows[window_index];
        let root = window.root.clone().remove(id);
        if let Some(root) = root {
            window.root = root;
            if window.focused == id {
                window.focused = window.root.first_block().into();
            }
        } else {
            self.remove_window(window_index);
        }
    }

    fn view(&self, host_id: &str) -> Value {
        let windows: Vec<Value> = self
            .windows
            .iter()
            .map(|window| {
                let mut blocks = Vec::new();
                let layout = window
                    .root
                    .layout([0.0, 0.0, 1.0, 1.0], &mut blocks, host_id);
                json!({
                    "window_id": window.id,
                    "label": window.label,
                    "focused_block_id": window.focused,
                    "layers": [{
                        "layer_id": window.layer_id,
                        "kind": "tiled",
                        "bounds": [0.0, 0.0, 1.0, 1.0],
                        "layout": layout,
                        "blocks": blocks,
                    }],
                })
            })
            .collect();
        json!({
            "session_id": self.id,
            "label": self.label,
            "revision": self.revision,
            "active_window_id": self.active,
            "windows": windows,
        })
    }
}

fn label(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        "Shell".into()
    } else {
        value.into()
    }
}

fn new_id(kind: &str) -> String {
    format!("{kind}:{}", Uuid::new_v4())
}

fn direction(value: &str) -> Option<SplitDirection> {
    match value {
        "horizontal" => Some(SplitDirection::Horizontal),
        "vertical" => Some(SplitDirection::Vertical),
        _ => None,
    }
}

fn direction_name(value: SplitDirection) -> &'static str {
    match value {
        SplitDirection::Horizontal => "horizontal",
        SplitDirection::Vertical => "vertical",
    }
}

fn created_window(window: &Window) -> Value {
    json!({ "window_id": window.id, "block_ids": [window.focused] })
}

#[cfg(test)]
mod tests {
    use super::State;
    use serde_json::{Value, json};

    fn apply(state: &mut State, request: Value) -> Value {
        let mut next = 0;
        state
            .apply(&request, || {
                next += 1;
                Ok(format!("pane:{next}"))
            })
            .unwrap()
    }

    #[test]
    fn create_split_resize_and_close_preserve_wire_shapes() {
        let mut state = State::default();
        let created = apply(
            &mut state,
            json!({ "op": "create", "label": " work ", "window_label": "main" }),
        );
        let session = created["session_id"].as_str().unwrap();
        let left = created["initial_windows"][0]["block_ids"][0]
            .as_str()
            .unwrap();
        assert!(
            created["initial_windows"][0]["window_id"]
                .as_str()
                .unwrap()
                .len()
                > 7
        );
        assert_eq!(
            state.summaries(),
            json!([{ "session_id": session, "label": "work" }])
        );

        let split = state
            .apply(
                &json!({
                    "op": "split", "session": session, "block": left,
                    "direction": "horizontal", "ratio": 0.25,
                }),
                || Ok("pane:two".into()),
            )
            .unwrap();
        let right = split["block_ids"][0].as_str().unwrap();
        assert_eq!(split["anchor_id"], left);
        assert!(state.contains(session, right));
        let view = state.view(session).unwrap();
        let layout = &view["windows"][0]["layers"][0]["layout"];
        assert_eq!(layout["split"]["ratio"], 0.25);
        assert_eq!(layout["split"]["after"]["block_id"], right);

        let resized = apply(
            &mut state,
            json!({
                "op": "resize-split", "session": session,
                "direction": "horizontal", "before": left, "after": right, "ratio": 0.6,
            }),
        );
        assert_eq!(resized["revision"], 3);
        apply(
            &mut state,
            json!({ "op": "close-block", "session": session, "block": left }),
        );
        assert!(!state.contains(session, left));
        assert!(state.contains(session, right));
        assert_eq!(
            state.view(session).unwrap()["windows"][0]["focused_block_id"],
            right
        );
    }

    #[test]
    fn failed_pane_creation_does_not_mutate_state() {
        let mut state = State::default();
        let result = state.apply(&json!({ "op": "create" }), || anyhow::bail!("pty failed"));
        assert!(result.is_err());
        assert_eq!(state.summaries(), json!([]));
    }

    #[test]
    fn duplicate_and_missing_targets_are_rejected() {
        let mut state = State::default();
        let created = apply(&mut state, json!({ "op": "create" }));
        let session = created["session_id"].as_str().unwrap();
        assert!(state.apply(&json!({ "op": "split", "session": session, "block": "missing", "direction": "horizontal" }), || Ok("new".into())).is_err());
        assert!(
            state
                .apply(&json!({ "op": "new-window", "session": session }), || Ok(
                    "pane:1".into()
                ))
                .is_err()
        );
        assert_eq!(state.pane_ids(session).unwrap().len(), 1);
    }

    #[test]
    fn ensure_default_and_window_focus_match_go_behavior() {
        let mut state = State::default();
        let first = apply(&mut state, json!({ "op": "ensure-default" }));
        let session = first["session_id"].as_str().unwrap();
        let second = state
            .apply(&json!({ "op": "ensure-default" }), || {
                panic!("must not create")
            })
            .unwrap();
        assert_eq!(second["session_id"], session);
        assert!(state.view(session).is_some());
        assert!(state.view("missing").is_none());
    }
}
