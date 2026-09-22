//! Band plans in the SDR++ JSON schema (D16): `{name, country_name,
//! country_code, author_name, author_url, bands:[{name, type, start, end}]}`,
//! frequencies in Hz. `type` is free text — SDR++'s own plans use some thirty
//! values — so it is kept as a string and the display decides its colour.

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct Band {
    pub name: String,
    pub kind: String,
    pub lo_hz: f64,
    pub hi_hz: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BandPlan {
    pub name: String,
    pub country_name: String,
    pub country_code: String,
    pub author: String,
    /// Sorted by `lo_hz`.
    pub bands: Vec<Band>,
    /// Bands dropped at load, with the reason: upstream plans carry typos
    /// (an end below its start) and a guessed correction would be a lie.
    pub rejected: Vec<String>,
}

#[derive(Deserialize)]
struct RawPlan {
    name: String,
    #[serde(default)]
    country_name: String,
    #[serde(default)]
    country_code: String,
    #[serde(default)]
    author_name: String,
    bands: Vec<RawBand>,
}

#[derive(Deserialize)]
struct RawBand {
    name: String,
    #[serde(rename = "type")]
    kind: String,
    start: f64,
    end: f64,
}

impl BandPlan {
    pub fn from_sdrpp_json(bytes: &[u8]) -> Result<Self, Error> {
        let raw: RawPlan = serde_json::from_slice(bytes)?;
        let mut bands = Vec::with_capacity(raw.bands.len());
        let mut rejected = Vec::new();
        for b in raw.bands {
            if !(b.start.is_finite() && b.end.is_finite() && b.start >= 0.0 && b.end > b.start) {
                rejected.push(format!("{}: {}..{} Hz", b.name, b.start, b.end));
                continue;
            }
            bands.push(Band {
                name: b.name,
                kind: b.kind,
                lo_hz: b.start,
                hi_hz: b.end,
            });
        }
        bands.sort_by(|a, b| a.lo_hz.total_cmp(&b.lo_hz));
        Ok(Self {
            name: raw.name,
            country_name: raw.country_name,
            country_code: raw.country_code,
            author: raw.author_name,
            bands,
            rejected,
        })
    }

    /// Every band containing `hz` (plans overlap: a ham band inside a
    /// broadcast allocation, a satellite sub-band inside a ham band).
    pub fn at(&self, hz: f64) -> impl Iterator<Item = &Band> {
        self.within(hz, hz)
    }

    /// Every band overlapping `lo..=hi`, in `lo_hz` order.
    pub fn within(&self, lo: f64, hi: f64) -> impl Iterator<Item = &Band> {
        // Bands starting above `hi` cannot overlap; the rest are checked
        // by their end (lengths vary, so no second binary search).
        let end = self.bands.partition_point(|b| b.lo_hz <= hi);
        self.bands[..end].iter().filter(move |b| b.hi_hz >= lo)
    }

    /// Whether the plan declares any DAB allocation. The shipped plans name
    /// them "DAB Radio", "T-DAB Broadcast", "DAB+ radio", "T-DAB", … — a
    /// name-only test, because the plans carry ranges, not a DAB kind.
    pub fn declares_dab(&self) -> bool {
        self.bands
            .iter()
            .any(|b| b.name.to_ascii_uppercase().contains("DAB"))
    }

    /// The Band III blocks whose centres this plan allocates for DAB, in
    /// raster order. Empty when the plan declares no DAB allocation — the
    /// blocks come from the plan, never from an invented per-country list.
    /// The L-band "T-DAB" allocations contain no Band III centre, so they
    /// contribute nothing here.
    pub fn dab_blocks(&self) -> Vec<crate::dab::DabBlock> {
        let allocations: Vec<&Band> = self
            .bands
            .iter()
            .filter(|b| b.name.to_ascii_uppercase().contains("DAB"))
            .collect();
        crate::dab::band_iii_blocks()
            .filter(|b| {
                allocations
                    .iter()
                    .any(|a| (a.lo_hz..=a.hi_hz).contains(&b.centre_hz))
            })
            .collect()
    }
}

/// A plan and its file stem.
pub type NamedPlan = (String, BandPlan);
/// A plan file that did not load.
pub type LoadError = (PathBuf, Error);

/// The plans in `shipped` and `user`, keyed by file stem and sorted by it;
/// a user plan shadows a shipped one with the same stem. A missing
/// directory is empty. Files that fail to parse come back as errors next to
/// the plans that loaded, so one bad file never hides the rest.
pub fn load_plans(shipped: &Path, user: &Path) -> (Vec<NamedPlan>, Vec<LoadError>) {
    let mut plans: Vec<NamedPlan> = Vec::new();
    let mut errors = Vec::new();
    for dir in [shipped, user] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut paths: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        paths.sort();
        for path in paths {
            let stem = path
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default();
            match std::fs::read(&path)
                .map_err(Error::from)
                .and_then(|b| BandPlan::from_sdrpp_json(&b))
            {
                Ok(plan) => {
                    plans.retain(|(k, _)| *k != stem);
                    plans.push((stem, plan));
                }
                Err(e) => errors.push((path, e)),
            }
        }
    }
    plans.sort_by(|a, b| a.0.cmp(&b.0));
    (plans, errors)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shipped() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/bandplans")
    }

    fn plan(json: &str) -> BandPlan {
        BandPlan::from_sdrpp_json(json.as_bytes()).unwrap()
    }

    #[test]
    fn every_shipped_plan_parses() {
        let (plans, errors) = load_plans(&shipped(), Path::new("/nonexistent"));
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(plans.len(), 21);
        for (stem, p) in &plans {
            assert!(!p.bands.is_empty(), "{stem}");
            assert!(p.bands.iter().all(|b| b.hi_hz > b.lo_hz), "{stem}");
        }
        // The upstream typos are dropped and reported, not repaired.
        let italy = &plans.iter().find(|(s, _)| s == "italy").unwrap().1;
        assert_eq!(italy.rejected.len(), 7, "{:?}", italy.rejected);
        assert!(italy.rejected[0].starts_with("GSM-R: "));
    }

    #[test]
    fn general_names_the_fm_band() {
        let (plans, _) = load_plans(&shipped(), Path::new("/nonexistent"));
        let general = &plans.iter().find(|(s, _)| s == "general").unwrap().1;
        let at: Vec<_> = general.at(99.4e6).map(|b| b.name.as_str()).collect();
        assert_eq!(at, ["FM Broadcast"]);
        assert_eq!(general.at(99.4e6).next().unwrap().kind, "broadcast");
    }

    #[test]
    fn overlapping_bands_both_come_back() {
        let p = plan(
            r#"{"name":"t","bands":[
              {"name":"wide","type":"broadcast","start":100,"end":200},
              {"name":"inner","type":"amateur","start":140,"end":160},
              {"name":"above","type":"marine","start":300,"end":400}]}"#,
        );
        let names = |it: &mut dyn Iterator<Item = &Band>| -> Vec<String> {
            it.map(|b| b.name.clone()).collect()
        };
        assert_eq!(names(&mut p.at(150.0)), ["wide", "inner"]);
        assert_eq!(names(&mut p.at(120.0)), ["wide"]);
        assert_eq!(names(&mut p.at(250.0)), Vec::<String>::new());
        assert_eq!(names(&mut p.within(190.0, 310.0)), ["wide", "above"]);
        // Edges are inclusive.
        assert_eq!(names(&mut p.at(200.0)), ["wide"]);
    }

    #[test]
    fn dab_blocks_are_the_plans_allocation_intersected_with_the_raster() {
        // The fixture: a DAB-named allocation (Belgium-like, 174–223 MHz)
        // and a plain broadcast band that must not contribute.
        let p = plan(
            r#"{"name":"t","bands":[
              {"name":"TV Broadcast","type":"broadcast","start":174000000,"end":230000000},
              {"name":"T-DAB Broadcast","type":"broadcast","start":174000000,"end":223000000}]}"#,
        );
        let blocks = p.dab_blocks();
        assert_eq!(blocks.first().unwrap().label, "5A");
        // 11D at 222.064 MHz is inside 223 MHz; 12A at 223.936 is not.
        assert_eq!(blocks.last().unwrap().label, "11D");
        assert!(blocks.iter().all(|b| b.centre_hz <= 223e6));
        assert!(p.declares_dab());

        // A plan with no DAB-named band says so with an empty list.
        let tv = plan(
            r#"{"name":"t","bands":[{"name":"TV Broadcast","type":"broadcast","start":174000000,"end":230000000}]}"#,
        );
        assert!(tv.dab_blocks().is_empty());
        assert!(!tv.declares_dab());
    }

    /// The operator's Band III allocation exists in the shipped plans, so
    /// the DAB channel selector has a list to offer out of the box: the
    /// Netherlands plan allocates 11C and 12C, and ends at 12D (230 MHz).
    #[test]
    fn the_shipped_netherlands_plan_allocates_the_11c_block() {
        let (plans, _) = load_plans(&shipped(), Path::new("/nonexistent"));
        let nl = &plans.iter().find(|(s, _)| s == "netherlands").unwrap().1;
        let labels: Vec<&str> = nl.dab_blocks().iter().map(|b| b.label).collect();
        assert!(labels.contains(&"11C"), "{labels:?}");
        assert!(labels.contains(&"12C"), "{labels:?}");
        assert_eq!(labels.last(), Some(&"12D"));
        // The fallback plan declares none: the app must say so, not invent.
        let general = &plans.iter().find(|(s, _)| s == "general").unwrap().1;
        assert!(general.dab_blocks().is_empty());
        assert!(!general.declares_dab());
    }

    #[test]
    fn a_user_plan_shadows_a_shipped_one() {
        let dir = std::env::temp_dir().join(format!("neowon-refdb-plans-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("general.json"),
            r#"{"name":"Mine","bands":[{"name":"x","type":"other","start":1,"end":2}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("broken.json"), "{").unwrap();
        let (plans, errors) = load_plans(&shipped(), &dir);
        let general = &plans.iter().find(|(s, _)| s == "general").unwrap().1;
        assert_eq!(general.name, "Mine");
        assert_eq!(plans.len(), 21);
        assert_eq!(errors.len(), 1);
        assert!(errors[0].0.ends_with("broken.json"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
