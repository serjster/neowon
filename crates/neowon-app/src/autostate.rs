//! Automatic state persistence (Phase 10.12, D13): the app saves itself to
//! `~/.neowon/state.nws` and comes back the way it was left.
//!
//! The file is an ordinary session script — the scope session (`session`)
//! plus UI scale, window size and position, dock sections, open windows
//! and the SDR settings — so it is readable, hand-editable and replayed by
//! the one script executor. Saving is debounced (the text is re-emitted
//! twice a second and written once it has been stable for `DEBOUNCE`) and
//! synchronous on exit; the write is atomic (`.tmp` + rename).
//!
//! Precedence is env > saved state > auto-fit: `NEOWON_UI_SCALE` and
//! `NEOWON_WINDOW` beat the saved geometry, which beats the monitor fit.
//! Persistence is off when `NEOWON_SCRIPT` or `NEOWON_NO_STATE` is set, so
//! regression runs stay deterministic; `NEOWON_STATE=<path>` moves the file.
//!
//! Left out on purpose: the sim stimulus and seed (test fixtures, and the
//! instrument switch resets them), and run/stop (an instrument that comes
//! up stopped looks broken). The workspace mode (SCOPE | SDR) **is** saved:
//! the launch flags still pick the family (simulators vs hardware), and the
//! saved mode picks which of the family's two instruments comes up
//! (operator, 2026-09-19).

use std::path::{Path, PathBuf};

use bevy::app::AppExit;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use neowon_backend::SdrGain;

use crate::Link;
use crate::script::{Action, Script};
use crate::sdr::{DabVerb, SdrAction, SdrState};

/// A change is written once the state has been stable this long, seconds.
const DEBOUNCE: f64 = 2.0;
/// How often the state is re-emitted and compared, seconds.
const CHECK_EVERY: f64 = 0.5;
/// Restore anyway if no instrument connects within this long, seconds
/// (the settings then wait in the app until one does).
const RESTORE_TIMEOUT: f64 = 3.0;

/// Where the state lives, or `None` when persistence is off.
pub fn path_from_env() -> Option<PathBuf> {
    if std::env::var_os("NEOWON_SCRIPT").is_some() || std::env::var_os("NEOWON_NO_STATE").is_some()
    {
        return None;
    }
    if let Some(p) = std::env::var_os("NEOWON_STATE") {
        return Some(p.into());
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))?;
    Some(Path::new(&home).join(".neowon/state.nws"))
}

#[derive(Resource, Default)]
pub struct AutoState {
    pub path: Option<PathBuf>,
    /// The saved text read at startup, replayed once an instrument
    /// connects. Nothing is saved before then, or the defaults would
    /// overwrite the file.
    saved: Option<String>,
    restored: bool,
    last_written: String,
    pending: String,
    pending_since: f64,
    next_check: f64,
    /// Last seen window geometry: the window is gone by the time the exit
    /// save runs.
    geometry: Option<(f32, f32, Option<IVec2>)>,
}

impl AutoState {
    pub fn from_env() -> Self {
        let path = path_from_env();
        let saved = path
            .as_deref()
            .and_then(|p| std::fs::read_to_string(p).ok());
        if let Some(p) = &path {
            info!(
                "state: {} ({})",
                p.display(),
                if saved.is_some() { "found" } else { "new" }
            );
        }
        Self {
            last_written: saved.clone().unwrap_or_default(),
            restored: saved.is_none(),
            saved,
            path,
            ..Default::default()
        }
    }

    /// Saved window geometry for `fit_display`: (UI scale, logical size,
    /// physical position).
    pub fn geometry(&self) -> Geometry {
        self.saved
            .as_deref()
            .map(Geometry::parse)
            .unwrap_or_default()
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Geometry {
    pub scale: Option<f32>,
    pub size: Option<(f32, f32)>,
    pub pos: Option<IVec2>,
}

impl Geometry {
    pub fn parse(text: &str) -> Self {
        let mut g = Geometry::default();
        for line in text.lines() {
            let mut w = line.split_whitespace();
            match (w.next(), w.next(), w.next()) {
                (Some("uiscale"), Some(s), None) => g.scale = s.parse().ok(),
                (Some("window"), Some(wh), None) => {
                    g.size = wh
                        .split_once('x')
                        .and_then(|(a, b)| Some((a.parse().ok()?, b.parse().ok()?)))
                }
                (Some("windowpos"), Some(x), Some(y)) => {
                    g.pos = x
                        .parse()
                        .ok()
                        .zip(y.parse().ok())
                        .map(|(x, y)| IVec2::new(x, y))
                }
                _ => {}
            }
        }
        g
    }
}

/// The UI-side resources the state records (one tuple keeps the
/// SystemParam readable).
type UiParts<'w> = (
    Res<'w, crate::ui::UiScale>,
    Res<'w, crate::ui::MenuState>,
    Res<'w, crate::ui::settings::Settings>,
    Res<'w, crate::catalog::CatalogState>,
    Res<'w, crate::refmap::RefMap>,
);

/// Everything the state file records.
#[derive(SystemParam)]
pub struct Snapshot<'w, 's> {
    link: ResMut<'w, Link>,
    sdr: Res<'w, SdrState>,
    ap: Res<'w, crate::autopeak::AutoPeak>,
    deep: Res<'w, crate::deep::DeepView>,
    rec: Res<'w, crate::record::Recorder>,
    phosphor: Res<'w, crate::gpu::Phosphor>,
    math: Res<'w, crate::derived::MathState>,
    meas: Res<'w, crate::derived::MeasureState>,
    fft: Res<'w, crate::derived::FftState>,
    cur: Res<'w, crate::cursors::CursorState>,
    pf: Res<'w, crate::derived::PfState>,
    wf: Res<'w, crate::viz::waterfall::WaterfallState>,
    viz: Res<'w, crate::viz::three_d::Viz3dState>,
    fx: Res<'w, crate::effects::Effects>,
    ui: UiParts<'w>,
    windows: Query<'w, 's, &'static Window>,
}

impl Snapshot<'_, '_> {
    fn window(&self) -> Option<(f32, f32, Option<IVec2>)> {
        let w = self.windows.single().ok()?;
        let pos = match w.position {
            bevy::window::WindowPosition::At(p) => Some(p),
            _ => None,
        };
        Some((w.width(), w.height(), pos))
    }

    fn emit(&self, geometry: Option<(f32, f32, Option<IVec2>)>) -> String {
        let (scale, menus, settings, catalog, refmap) = &self.ui;
        let mut s = String::from("# neowon state — saved automatically; replayed at launch\n");
        s += &format!("uiscale {}\n", scale.0);
        if let Some((w, h, pos)) = geometry {
            s += &format!("window {}x{}\n", w.round(), h.round());
            if let Some(p) = pos {
                s += &format!("windowpos {} {}\n", p.x, p.y);
            }
        }
        let open: Vec<&str> = menus.open_list().iter().map(|m| m.name()).collect();
        let open = if open.is_empty() {
            "none".to_string()
        } else {
            open.join(",")
        };
        s += &format!("dock {open}\n");
        let on = |b: bool| if b { "on" } else { "off" };
        s += &format!("measwin {}\n", on(self.meas.window));
        s += &format!("settings {}\n", on(settings.open));
        s += &format!("catalog window {}\n", on(catalog.window));

        let session = crate::session::emit(
            &self.ap,
            &self.deep,
            self.rec.budget,
            &self.link,
            &self.phosphor,
            &self.math,
            &self.meas,
            &self.fft,
            &self.cur,
            &self.pf,
            &self.wf,
            &self.viz,
            &self.fx,
        );
        for line in session.lines().filter(|l| !l.starts_with('#')) {
            if !(line.starts_with("stimulus ") || line.starts_with("run ")) {
                s += line;
                s.push('\n');
            }
        }
        for a in sdr_actions(&self.sdr) {
            s += &format!("{a}\n");
        }
        // Last, after both modes' settings: the switch then hands the new
        // instrument the config the lines above just restored. Within the
        // launch's family, so a saved SDR mode cannot turn a `--sim` run
        // into hardware or vice versa.
        s += &format!(
            "instrument {}\n",
            if self.sdr.active { "sdr" } else { "scope" }
        );
        use crate::refmap::RefMapAction as R;
        for a in [
            R::Plan(refmap.stem().to_string()),
            R::Strip(refmap.strip),
            R::Mini(refmap.mini),
            R::Window(refmap.window),
        ] {
            s += &format!("{a}\n");
        }
        s
    }
}

/// The SDR settings as the actions that recreate them. Tune before
/// centre: centre then leaves Tuned where it was even when the saved
/// window no longer covers it.
pub fn sdr_actions(sdr: &SdrState) -> Vec<SdrAction> {
    let c = &sdr.config;
    vec![
        SdrAction::Rate(c.sample_rate),
        SdrAction::Gain(match c.gain {
            SdrGain::Auto => None,
            SdrGain::Manual(db) => Some(db),
        }),
        SdrAction::Agc(c.agc),
        SdrAction::Ppm(c.ppm),
        SdrAction::Tune(sdr.tuned_hz),
        SdrAction::Centre(c.centre_hz),
        SdrAction::Follow(sdr.follow),
        SdrAction::Span(sdr.span_hz),
        SdrAction::Pan(sdr.pan_hz),
        SdrAction::Fft(sdr.fft_size),
        SdrAction::Level {
            ref_db: sdr.ref_db,
            range_db: sdr.range_db,
        },
        SdrAction::Detect(sdr.detect_on),
        SdrAction::Threshold(sdr.threshold_db),
        SdrAction::Analyse(sdr.analyse_on),
        SdrAction::Modulation(sdr.modulation),
        SdrAction::Width((!sdr.width_auto).then_some(sdr.width_hz)),
        SdrAction::Demod(sdr.demod),
        // Saved only when it is on: a restored session should not start
        // decoding an ensemble nobody asked for.
        SdrAction::Dab(match sdr.dab {
            Some(_) => DabVerb::On,
            None => DabVerb::Off,
        }),
        SdrAction::Volume(sdr.volume),
        SdrAction::Mute(sdr.mute),
        SdrAction::Squelch((sdr.squelch_db > -120.0).then_some(sdr.squelch_db)),
        SdrAction::List(sdr.list_px),
    ]
}

/// Replay the saved state once an instrument is connected, then keep the
/// file current.
pub fn tick(
    time: Res<Time>,
    mut st: ResMut<AutoState>,
    mut script: ResMut<Script>,
    mut snap: Snapshot,
    mut exit: MessageReader<AppExit>,
) {
    if st.path.is_none() {
        return;
    }
    let now = time.elapsed_secs_f64();
    if let Some(g) = snap.window() {
        st.geometry = Some(g);
    }
    if !st.restored {
        let connected = if snap.sdr.active {
            snap.sdr.caps.is_some()
        } else {
            snap.link.caps.is_some()
        };
        if connected || now > RESTORE_TIMEOUT {
            restore(&mut st, &mut script, &mut snap);
        }
        return;
    }
    let exiting = exit.read().count() > 0;
    if !exiting && now < st.next_check {
        return;
    }
    st.next_check = now + CHECK_EVERY;
    let text = snap.emit(st.geometry);
    if text == st.last_written {
        return;
    }
    if text != st.pending {
        st.pending = text.clone();
        st.pending_since = now;
    }
    if exiting || now - st.pending_since >= DEBOUNCE {
        write(&mut st, text);
    }
}

fn restore(st: &mut AutoState, script: &mut Script, snap: &mut Snapshot) {
    st.restored = true;
    let Some(text) = st.saved.take() else { return };
    // Geometry was applied at startup, where env overrides win.
    let body: String = text
        .lines()
        .filter(|l| {
            let verb = l.split_whitespace().next().unwrap_or("");
            !matches!(verb, "uiscale" | "window" | "windowpos")
        })
        .map(|l| format!("{l}\n"))
        .collect();
    match crate::script::parse(&body) {
        Ok(actions) => {
            let mut dropped = Vec::new();
            let n = actions.len();
            for (_, a) in actions {
                match invalid(&a, snap) {
                    Some(why) => dropped.push(why),
                    None => script.inject(a),
                }
            }
            info!("state: restored {} settings", n - dropped.len());
            if !dropped.is_empty() {
                let msg = format!("state: dropped {}", dropped.join("; "));
                warn!("{msg}");
                snap.link.status = msg;
            }
        }
        Err(e) => {
            let msg = format!("state: not restored ({e})");
            warn!("{msg}");
            snap.link.status = msg;
        }
    }
}

/// Saved values the attached instrument cannot take (D13). The SDR's own
/// actions refuse out-of-range frequencies with a status line already;
/// the scope's time base ladder is checked here.
fn invalid(a: &Action, snap: &Snapshot) -> Option<String> {
    match a {
        Action::Rate(r) => {
            let caps = snap.link.caps.as_ref()?;
            (!caps.sample_rates.iter().any(|x| (x - r).abs() <= r * 1e-9))
                .then(|| format!("sample rate {r} S/s (not offered by {})", caps.name))
        }
        _ => None,
    }
}

fn write(st: &mut AutoState, text: String) {
    let Some(path) = st.path.clone() else { return };
    match write_atomic(&path, &text) {
        Ok(()) => debug!("state: saved {}", path.display()),
        Err(e) => warn!("state: cannot save {}: {e}", path.display()),
    }
    st.last_written = text;
}

fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("nws.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_reads_the_saved_lines() {
        let g = Geometry::parse("# x\nuiscale 1\nwindow 1600x900\nwindowpos -10 25\ndock none\n");
        assert_eq!(
            g,
            Geometry {
                scale: Some(1.0),
                size: Some((1600.0, 900.0)),
                pos: Some(IVec2::new(-10, 25)),
            }
        );
        assert_eq!(Geometry::parse("window big\n"), Geometry::default());
    }

    #[test]
    fn sdr_actions_replay_to_the_same_state() {
        let mut a = SdrState::default();
        a.config.centre_hz = 145e6;
        a.tuned_hz = 146.2e6; // outside the saved window
        a.config.gain = SdrGain::Manual(28.0);
        a.span_hz = 200e3;
        a.pan_hz = -300e3;
        a.width_auto = false;
        a.width_hz = 15e3;
        a.demod = Some(neowon_dsp::DemodMode::Wfm);
        a.squelch_db = -60.0;
        a.volume = 0.25;

        let text: String = sdr_actions(&a).iter().map(|x| format!("{x}\n")).collect();
        let mut b = SdrState::default();
        let mut link = Link {
            sup: neowon_backend::spawn(|| {
                Ok(Box::new(neowon_sim::SimSdrBackend::new()) as Box<dyn neowon_backend::Backend>)
            }),
            caps: None,
            status: String::new(),
            latest: None,
            config: Default::default(),
            dirty: false,
            frames_seen: 0,
            multi: neowon_backend::MultiMode::TriggerOut,
            last_frame_at: 0.0,
            arrived: Vec::new(),
            stimulus: String::new(),
            selected: 0,
        };
        for line in text.lines() {
            let mut w = line.split_whitespace();
            w.next(); // "sdr"
            let act = crate::sdr::parse(&mut || w.next().ok_or_else(|| "eol".to_string())).unwrap();
            crate::sdr::run(act, &mut b, &mut link);
        }
        assert_eq!(b.tuned_hz, a.tuned_hz);
        assert_eq!(b.config.centre_hz, a.config.centre_hz);
        assert_eq!(b.config.gain, a.config.gain);
        assert_eq!((b.span_hz, b.pan_hz), (a.span_hz, a.pan_hz));
        assert_eq!((b.width_auto, b.width_hz), (false, 15e3));
        assert_eq!(b.demod, a.demod);
        assert_eq!((b.squelch_db, b.volume), (-60.0, 0.25));
        assert!(link.status.is_empty(), "{}", link.status);
        link.sup.shutdown();
    }
}
