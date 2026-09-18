//! Control-socket readouts for the catalog: `get catalog`, `get history`.

use neowon_catalog::Id;

use super::CatalogState;

fn esc(s: &str) -> String {
    format!("\"{}\"", crate::control::escape(s))
}

/// `get catalog`: the listed signals and the catalog's health.
pub fn catalog_json(st: &CatalogState) -> String {
    let Some(cat) = &st.cat else {
        return format!(
            r#"{{"ok":false,"error":"no catalog open at {}"}}"#,
            st.path.display()
        );
    };
    let rows: Vec<String> = st
        .signals()
        .iter()
        .map(|s| {
            let tags: Vec<String> = s.tags.iter().map(|t| esc(t)).collect();
            let aliases: Vec<String> = s.aliases.iter().map(|a| esc(&a.name)).collect();
            format!(
                concat!(
                    r#"{{"id":{},"name":{},"centre_hz":{},"bandwidth_hz":{},"tags":[{}],"#,
                    r#""aliases":[{}],"pinned":{},"observations":{}}}"#
                ),
                s.id.0,
                esc(&s.name),
                s.centre_hz,
                s.bandwidth_hz,
                tags.join(","),
                aliases.join(","),
                s.pinned,
                st.observations_of(s.id)
            )
        })
        .collect();
    format!(
        r#"{{"ok":true,"path":{},"seq":{},"entities":{},"integrity":{},"signals":[{}]}}"#,
        esc(&st.path.display().to_string()),
        cat.seq(),
        cat.state().entities.len(),
        cat.state().integrity().len(),
        rows.join(",")
    )
}

/// `get history <id>`: a signal's observations, redirects followed.
pub fn history_json(st: &CatalogState, id: &str) -> String {
    let (Some(cat), Ok(id)) = (&st.cat, id.parse::<Id>()) else {
        return r#"{"ok":false,"error":"no catalog, or a bad id"}"#.into();
    };
    match cat.state().history(id) {
        Ok(h) => {
            let rows: Vec<String> = h
                .iter()
                .map(|o| {
                    format!(
                        r#"{{"id":{},"signal":{},"t_start":{},"centre_hz":{},"power_dbfs":{},"snr_db":{}}}"#,
                        o.id.0, o.signal.0, o.obs.t_start, o.obs.centre_hz, o.obs.power_dbfs, o.obs.snr_db
                    )
                })
                .collect();
            format!(
                r#"{{"ok":true,"canonical":{},"rows":[{}]}}"#,
                cat.state().resolve(id).map_or(0, |i| i.0),
                rows.join(",")
            )
        }
        Err(e) => format!(r#"{{"ok":false,"error":{}}}"#, esc(&e.to_string())),
    }
}
