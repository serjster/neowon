//! `get uitree` as a flat list of nodes, for geometry assertions: every
//! widget and custom-painted element with its role, label, value and
//! `[x, y, w, h]` rect (physical pixels, the tree's own units).

/// One node of the tree. `label` and `value` are empty when absent.
#[derive(Debug, Clone)]
pub struct Node {
    pub role: String,
    pub label: String,
    pub value: String,
    pub rect: [f32; 4],
    pub disabled: bool,
}

impl Node {
    /// Label, or value when the node has no label (egui labels carry their
    /// text as the value).
    pub fn text(&self) -> &str {
        if self.label.is_empty() {
            &self.value
        } else {
            &self.label
        }
    }
    pub fn right(&self) -> f32 {
        self.rect[0] + self.rect[2]
    }
    pub fn bottom(&self) -> f32 {
        self.rect[1] + self.rect[3]
    }
    /// True when the node lies inside `outer`'s rect, to half a pixel.
    pub fn inside(&self, outer: &Node) -> bool {
        self.rect[0] >= outer.rect[0] - 0.5
            && self.rect[1] >= outer.rect[1] - 0.5
            && self.right() <= outer.right() + 0.5
            && self.bottom() <= outer.bottom() + 0.5
    }
}

/// A JSON string starting just after its opening quote: the text, unescaped.
fn string_at(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some('u') => {
                    let hex: String = chars.by_ref().take(4).collect();
                    if let Some(ch) = u32::from_str_radix(&hex, 16).ok().and_then(char::from_u32) {
                        out.push(ch);
                    }
                }
                Some(other) => out.push(other),
                None => break,
            },
            c => out.push(c),
        }
    }
    out
}

/// Every node that carries a rect, in tree order (nested children and the
/// trailing `regions` included).
pub fn nodes(tree: &str) -> Vec<Node> {
    const OPEN: &str = r#"{"role":""#;
    let starts: Vec<usize> = tree.match_indices(OPEN).map(|(i, _)| i).collect();
    let mut out = Vec::new();
    for (k, &at) in starts.iter().enumerate() {
        // A node's own fields all precede its `children`, i.e. the next node.
        let end = starts.get(k + 1).copied().unwrap_or(tree.len());
        let chunk = &tree[at + OPEN.len()..end];
        let role = string_at(chunk);
        let field = |key: &str| {
            let pat = format!(r#""{key}":""#);
            chunk
                .find(&pat)
                .map(|i| string_at(&chunk[i + pat.len()..]))
                .unwrap_or_default()
        };
        let Some(r) = chunk.find(r#""rect":["#) else {
            continue;
        };
        let inner = &chunk[r + 8..];
        let inner = &inner[..inner.find(']').unwrap_or(inner.len())];
        let v: Vec<f32> = inner
            .split(',')
            .filter_map(|x| x.trim().parse().ok())
            .collect();
        if v.len() != 4 {
            continue;
        }
        out.push(Node {
            role,
            label: field("label"),
            value: field("value"),
            rect: [v[0], v[1], v[2], v[3]],
            disabled: chunk.contains(r#""disabled":true"#),
        });
    }
    out
}

/// The first node whose label or value starts with `prefix`.
pub fn find<'a>(nodes: &'a [Node], prefix: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.text().starts_with(prefix))
}

/// The first node with this role and exact label or value.
pub fn exact<'a>(nodes: &'a [Node], role: &str, text: &str) -> Option<&'a Node> {
    nodes.iter().find(|n| n.role == role && n.text() == text)
}
