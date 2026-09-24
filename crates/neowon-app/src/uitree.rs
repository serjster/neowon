//! The UI element tree — the app's "DOM inspector". egui already builds an
//! AccessKit tree of every widget each frame (role, label, value, state,
//! bounds); this module turns it on, keeps the last one, and exports it as
//! JSON, so layout can be audited and asserted from structure instead of
//! screenshots. Custom-painted elements (canvases, band segments, dock
//! sections drawn by hand) join the tree through `node`; the painted
//! regions every frame records in `UiRects` are attached as `Region` nodes.
//!
//! Reach it with `uitree <path>` (script), `get uitree` (control socket)
//! or the MCP `ui_tree` tool. Rects are `[x, y, w, h]` in logical window
//! pixels — the units of the `layout` dump.

use bevy::prelude::*;
use bevy_egui::egui::accesskit::{self, Node, NodeId, TreeUpdate};
use bevy_egui::{EguiContext, EguiOutput, PrimaryEguiContext, egui};
use std::collections::HashMap;
use std::fmt::Write;

use crate::control::escape;
use crate::ui::layout::{Layout, UiRects};

#[derive(Resource, Default)]
pub struct UiTree {
    /// Build the tree every frame (costs a little; on while a control
    /// socket is open, or once a `uitree` is asked for).
    pub on: bool,
    enabled: bool,
    last: Option<TreeUpdate>,
    /// `uitree <path>` requests waiting for the next tree.
    pub pending: Vec<String>,
}

impl UiTree {
    pub fn from_env() -> Self {
        Self {
            on: crate::control::configured_port().is_some(),
            ..Default::default()
        }
    }

    pub fn json(&self, layout: &Layout, rects: &UiRects) -> Option<String> {
        self.last.as_ref().map(|t| render(t, layout, rects))
    }
}

/// Add a custom-painted element to the tree (a no-op while the tree is
/// off). `rect` is in egui points, like every egui rect.
pub fn node(
    ctx: &egui::Context,
    id: egui::Id,
    role: accesskit::Role,
    label: &str,
    rect: egui::Rect,
) {
    ctx.accesskit_node_builder(id, |n| {
        n.set_role(role);
        n.set_label(label);
        n.set_bounds(accesskit::Rect {
            x0: rect.min.x.into(),
            y0: rect.min.y.into(),
            x1: rect.max.x.into(),
            y1: rect.max.y.into(),
        });
    });
}

/// Turn AccessKit output on for the primary context once asked to.
pub fn enable(
    mut tree: ResMut<UiTree>,
    mut ctx: Query<&mut EguiContext, With<PrimaryEguiContext>>,
) {
    if tree.on && !tree.enabled {
        for mut c in &mut ctx {
            c.get_mut().enable_accesskit();
            tree.enabled = true;
        }
    }
}

/// Keep the frame's tree and answer waiting `uitree` requests.
pub fn capture(
    mut tree: ResMut<UiTree>,
    out: Query<&EguiOutput, With<PrimaryEguiContext>>,
    layout: Res<Layout>,
    rects: Res<UiRects>,
) {
    let Some(update) = out
        .iter()
        .find_map(|o| o.platform_output.accesskit_update.clone())
    else {
        return;
    };
    tree.last = Some(update);
    for path in std::mem::take(&mut tree.pending) {
        let json = tree.json(&layout, &rects).unwrap_or_default();
        match neowon_core::atomic_file::write(&path, json) {
            Ok(()) => info!("uitree: wrote {path}"),
            Err(e) => error!("uitree: cannot write {path}: {e}"),
        }
    }
}

/// The tree as nested JSON. egui parents a custom node to the root when it
/// cannot see its Ui; a node listed under any other parent is taken from
/// the root, so each node appears exactly once.
fn render(t: &TreeUpdate, layout: &Layout, rects: &UiRects) -> String {
    let nodes: HashMap<NodeId, &Node> = t.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let root = t.tree.as_ref().map(|tr| tr.root).unwrap_or(t.focus);
    let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
    for (id, n) in &t.nodes {
        for c in n.children() {
            match parent.get(c) {
                Some(p) if *p != root => {}
                _ => {
                    parent.insert(*c, *id);
                }
            }
        }
    }
    let scale = layout.scale;
    let mut s = String::new();
    let _ = write!(
        s,
        r#"{{"ok":true,"scale":{scale},"window":[{},{}],"root":"#,
        layout.window.x, layout.window.y
    );
    let root_json = nodes.get(&root).map(|n| {
        let kids: Vec<String> = n
            .children()
            .iter()
            .filter(|c| parent.get(c) == Some(&root))
            .flat_map(|c| emit(*c, &nodes, &parent, scale, 1))
            .collect();
        format!(r#"{{"role":"Window","children":[{}]}}"#, kids.join(","))
    });
    s.push_str(root_json.as_deref().unwrap_or("null"));
    s.push_str(r#","regions":["#);
    let regions = rects
        .regions
        .iter()
        .map(|(n, r)| (n.to_string(), *r))
        .chain(rects.floating.iter().cloned());
    for (i, (name, r)) in regions.enumerate() {
        if i > 0 {
            s.push(',');
        }
        let _ = write!(
            s,
            r#"{{"role":"Region","label":"{}","rect":[{:.1},{:.1},{:.1},{:.1}]}}"#,
            escape(&name),
            r.min.x,
            r.min.y,
            r.width(),
            r.height()
        );
    }
    s.push_str("]}");
    s
}

/// The JSON objects `id` contributes to its parent's `children`: itself,
/// or — for an unnamed, boundless container, which only groups — its
/// children in its place. Text runs (the glyph lines inside a label) are
/// dropped: the label carries their text.
fn emit(
    id: NodeId,
    nodes: &HashMap<NodeId, &Node>,
    parent: &HashMap<NodeId, NodeId>,
    scale: f32,
    depth: usize,
) -> Vec<String> {
    let Some(n) = nodes.get(&id) else {
        return Vec::new();
    };
    // Guard against a malformed tree ever recursing without end.
    let kids: Vec<String> = if depth < 64 {
        n.children()
            .iter()
            .filter(|c| parent.get(c) == Some(&id))
            .flat_map(|c| emit(*c, nodes, parent, scale, depth + 1))
            .collect()
    } else {
        Vec::new()
    };
    let label = n.label().filter(|l| !l.is_empty());
    match n.role() {
        accesskit::Role::TextRun => return Vec::new(),
        accesskit::Role::GenericContainer if label.is_none() && n.bounds().is_none() => {
            return kids;
        }
        _ => {}
    }
    let mut s = String::new();
    let _ = write!(s, r#"{{"role":"{:?}""#, n.role());
    if let Some(l) = label {
        let _ = write!(s, r#","label":"{}""#, escape(l));
    }
    if let Some(v) = n.value().filter(|v| !v.is_empty()) {
        let _ = write!(s, r#","value":"{}""#, escape(v));
    }
    if let Some(v) = n.numeric_value() {
        let _ = write!(s, r#","numeric":{v}"#);
    }
    if let Some(t) = n.toggled() {
        let _ = write!(s, r#","toggled":"{t:?}""#);
    }
    if n.is_disabled() {
        s.push_str(r#","disabled":true"#);
    }
    if n.supports_action(accesskit::Action::Click) {
        s.push_str(r#","clickable":true"#);
    }
    if let Some(b) = n.bounds() {
        let k = scale as f64;
        let _ = write!(
            s,
            r#","rect":[{:.1},{:.1},{:.1},{:.1}]"#,
            b.x0 * k,
            b.y0 * k,
            (b.x1 - b.x0) * k,
            (b.y1 - b.y0) * k
        );
    }
    if !kids.is_empty() {
        let _ = write!(s, r#","children":[{}]"#, kids.join(","));
    }
    s.push('}');
    vec![s]
}

/// Name the container `ui` draws into, so it shows up in the tree as a
/// region with its bounds instead of an anonymous group.
pub fn name(ui: &egui::Ui, label: &str) {
    node(
        ui.ctx(),
        ui.id(),
        accesskit::Role::Group,
        label,
        ui.max_rect(),
    );
}
