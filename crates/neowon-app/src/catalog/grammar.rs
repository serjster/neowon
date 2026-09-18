//! The `catalog …` script grammar: parse a line into a `CatalogAction`,
//! and print one back (`Display`), so the window's actions and scripts
//! are the same thing.

use neowon_catalog::Id;

#[derive(Debug, Clone, PartialEq)]
pub enum CatalogAction {
    List(String),
    /// A signal: explicit (`centre_hz`, optional name) or, with no
    /// arguments, the strongest live detection.
    Add(Option<(f64, String)>),
    Observe,
    Rename(Id, String),
    Delete(Id, bool),
    Purge(Vec<Id>, bool),
    Merge(Id, Id),
    Tag(Id, String, bool),
    Alias(Id, String),
    Edit(Id, String, String),
    /// `bulk tag|untag|pin|unpin|delete <ids> [tag]`.
    Bulk(String, Vec<Id>, String),
    Undo,
    Pin(Id, bool),
    Export(String),
    Import(String),
    Window(bool),
    Select(Option<Id>),
    /// File the latest completed survey's coverage (optional name).
    Survey(String),
}

fn ids(s: &str) -> Result<Vec<Id>, String> {
    s.split(',')
        .filter(|x| !x.is_empty())
        .map(|x| x.parse())
        .collect()
}

/// `catalog <verb> …` (the words after `catalog`).
pub fn parse<'a>(
    next: &mut dyn FnMut() -> Result<&'a str, String>,
) -> Result<CatalogAction, String> {
    let verb = next()?;
    let mut rest = Vec::new();
    while let Ok(w) = next() {
        rest.push(w);
    }
    let arg = |i: usize| {
        rest.get(i)
            .copied()
            .ok_or_else(|| format!("catalog {verb}: missing argument"))
    };
    let tail = |i: usize| rest.get(i..).map(|r| r.join(" ")).unwrap_or_default();
    let id = |i: usize| -> Result<Id, String> { arg(i)?.parse() };
    let cascade = || rest.contains(&"cascade");
    Ok(match verb {
        "list" => CatalogAction::List(tail(0)),
        "add" if rest.is_empty() => CatalogAction::Add(None),
        "add" => CatalogAction::Add(Some((crate::sdr::parse_hz(arg(0)?)?, tail(1)))),
        "observe" => CatalogAction::Observe,
        "rename" => CatalogAction::Rename(id(0)?, tail(1)),
        "delete" => CatalogAction::Delete(id(0)?, cascade()),
        "purge" => CatalogAction::Purge(ids(arg(0)?)?, cascade()),
        "merge" => CatalogAction::Merge(id(0)?, id(1)?),
        "tag" => CatalogAction::Tag(id(0)?, arg(1)?.to_string(), true),
        "untag" => CatalogAction::Tag(id(0)?, arg(1)?.to_string(), false),
        "alias" => CatalogAction::Alias(id(0)?, tail(1)),
        "edit" => CatalogAction::Edit(id(0)?, arg(1)?.to_string(), tail(2)),
        "bulk" => CatalogAction::Bulk(arg(0)?.to_string(), ids(arg(1)?)?, tail(2)),
        "undo" => CatalogAction::Undo,
        "pin" => CatalogAction::Pin(id(0)?, true),
        "unpin" => CatalogAction::Pin(id(0)?, false),
        "export" => CatalogAction::Export(tail(0)),
        "import" => CatalogAction::Import(tail(0)),
        "window" => CatalogAction::Window(matches!(arg(0)?, "on" | "1")),
        "select" => CatalogAction::Select(arg(0)?.parse().ok()),
        "survey" => CatalogAction::Survey(tail(0)),
        other => return Err(format!("unknown catalog verb {other:?}")),
    })
}

/// The script line for an action (`catalog …`). Every variant has one
/// (the match is exhaustive) and `parse` reads it back to the same action:
/// the Catalog window injects these actions, so this is the script-parity
/// rule by construction. Names and paths keep single spaces.
impl std::fmt::Display for CatalogAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let list = |v: &[Id]| v.iter().map(Id::to_string).collect::<Vec<_>>().join(",");
        let cascade = |c: bool| if c { " cascade" } else { "" };
        write!(f, "catalog ")?;
        match self {
            CatalogAction::List(filter) => write!(f, "list {filter}"),
            CatalogAction::Add(None) => write!(f, "add"),
            CatalogAction::Add(Some((hz, name))) => write!(f, "add {hz} {name}"),
            CatalogAction::Observe => write!(f, "observe"),
            CatalogAction::Rename(id, name) => write!(f, "rename {id} {name}"),
            CatalogAction::Delete(id, c) => write!(f, "delete {id}{}", cascade(*c)),
            CatalogAction::Purge(ids, c) => write!(f, "purge {}{}", list(ids), cascade(*c)),
            CatalogAction::Merge(from, to) => write!(f, "merge {from} {to}"),
            CatalogAction::Tag(id, tag, true) => write!(f, "tag {id} {tag}"),
            CatalogAction::Tag(id, tag, false) => write!(f, "untag {id} {tag}"),
            CatalogAction::Alias(id, name) => write!(f, "alias {id} {name}"),
            CatalogAction::Edit(id, field, value) => write!(f, "edit {id} {field} {value}"),
            CatalogAction::Bulk(op, ids, arg) => write!(f, "bulk {op} {} {arg}", list(ids)),
            CatalogAction::Undo => write!(f, "undo"),
            CatalogAction::Pin(id, true) => write!(f, "pin {id}"),
            CatalogAction::Pin(id, false) => write!(f, "unpin {id}"),
            CatalogAction::Export(path) => write!(f, "export {path}"),
            CatalogAction::Import(path) => write!(f, "import {path}"),
            CatalogAction::Window(on) => write!(f, "window {}", if *on { "on" } else { "off" }),
            CatalogAction::Select(None) => write!(f, "select none"),
            CatalogAction::Select(Some(id)) => write!(f, "select {id}"),
            CatalogAction::Survey(name) => write!(f, "survey {name}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words<'a>(s: &'a str) -> impl FnMut() -> Result<&'a str, String> {
        let mut w = s.split_whitespace();
        move || w.next().ok_or_else(|| "missing argument".to_string())
    }

    #[test]
    fn verbs_parse() {
        assert_eq!(parse(&mut words("add")).unwrap(), CatalogAction::Add(None));
        assert_eq!(
            parse(&mut words("add 99.4M BBC Radio 2")).unwrap(),
            CatalogAction::Add(Some((99.4e6, "BBC Radio 2".into())))
        );
        assert_eq!(
            parse(&mut words("rename #3 Radio Two")).unwrap(),
            CatalogAction::Rename(Id(3), "Radio Two".into())
        );
        assert_eq!(
            parse(&mut words("purge 1,2,3 cascade")).unwrap(),
            CatalogAction::Purge(vec![Id(1), Id(2), Id(3)], true)
        );
        assert_eq!(
            parse(&mut words("merge 4 #5")).unwrap(),
            CatalogAction::Merge(Id(4), Id(5))
        );
        assert_eq!(
            parse(&mut words("bulk tag 1,2 fm")).unwrap(),
            CatalogAction::Bulk("tag".into(), vec![Id(1), Id(2)], "fm".into())
        );
        assert!(parse(&mut words("frobnicate")).is_err());
        assert!(parse(&mut words("merge 4")).is_err());
    }

    /// Which variant an action is. Exhaustive, so a new variant fails to
    /// compile here until `every_action_round_trips` covers it.
    fn variant(a: &CatalogAction) -> usize {
        use CatalogAction::*;
        match a {
            List(_) => 0,
            Add(_) => 1,
            Observe => 2,
            Rename(..) => 3,
            Delete(..) => 4,
            Purge(..) => 5,
            Merge(..) => 6,
            Tag(..) => 7,
            Alias(..) => 8,
            Edit(..) => 9,
            Bulk(..) => 10,
            Undo => 11,
            Pin(..) => 12,
            Export(_) => 13,
            Import(_) => 14,
            Window(_) => 15,
            Select(_) => 16,
            Survey(_) => 17,
        }
    }

    /// Script parity: every action the Catalog window can inject prints as
    /// a script line that parses back to the same action.
    #[test]
    fn every_action_round_trips() {
        use CatalogAction::*;
        let all = [
            List(String::new()),
            List("fm radio".into()),
            Add(None),
            Add(Some((99.412_5e6, "Radio Two".into()))),
            Add(Some((433.92e6, String::new()))),
            Observe,
            Rename(Id(3), "Radio Two".into()),
            Delete(Id(4), false),
            Delete(Id(4), true),
            Purge(vec![Id(1), Id(2)], true),
            Merge(Id(4), Id(5)),
            Tag(Id(2), "fm".into(), true),
            Tag(Id(2), "fm".into(), false),
            Alias(Id(2), "R2".into()),
            Edit(Id(2), "notes".into(), "strong at night".into()),
            Bulk("pin".into(), vec![Id(1), Id(7)], String::new()),
            Bulk("tag".into(), vec![Id(1)], "fm".into()),
            Undo,
            Pin(Id(9), true),
            Pin(Id(9), false),
            Export("/tmp/cat export.json".into()),
            Import("/tmp/cat.json".into()),
            Window(true),
            Window(false),
            Select(None),
            Select(Some(Id(12))),
            Survey("band scan".into()),
        ];
        let mut seen = std::collections::BTreeSet::new();
        for a in all {
            seen.insert(variant(&a));
            let line = a.to_string();
            let rest = line.strip_prefix("catalog ").expect("catalog verb");
            assert_eq!(parse(&mut words(rest)).unwrap(), a, "{line}");
        }
        assert_eq!(seen.len(), 18, "a variant has no round-trip sample");
    }
}
