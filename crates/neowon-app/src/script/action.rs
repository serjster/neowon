//! The script vocabulary: one `Action` per verb the grammar parses, shared by
//! scripts, the UI and the control socket.

use neowon_backend::MultiMode;
use neowon_core::{AcqMode, Coupling, PulseCondition, Slope, Sweep, VideoSync};
use neowon_dsp::{MathOp, Window};

use crate::gpu::{Persistence, TraceMode};
use crate::ui::Menu;

#[derive(Debug, Clone)]
pub enum Action {
    Stimulus(String),
    Sdr(crate::sdr::SdrAction),
    Catalog(crate::catalog::CatalogAction),
    Rate(f64),
    Vdiv(usize, f64),
    Enable(usize, bool),
    CouplingSet(usize, Coupling),
    Probe(usize, f64),
    Offset(usize, f64),
    Trigger {
        ch: usize,
        slope: Slope,
        level: f64,
        sweep: Sweep,
    },
    TrigPulse {
        ch: usize,
        cond: PulseCondition,
        width: f64,
        sweep: Sweep,
    },
    TrigSlope {
        ch: usize,
        cond: PulseCondition,
        width: f64,
        upper: f64,
        lower: f64,
        sweep: Sweep,
    },
    TrigVideo {
        sync: VideoSync,
        line: u16,
        sweep: Sweep,
    },
    Holdoff(f64),
    AutoSet,
    Force,
    Zoom {
        horiz: bool,
        inward: bool,
    },
    HZoom {
        inward: bool,
    },
    HView(f64, f64),
    /// Time base in seconds per division (the primary horizontal control).
    Timebase(f64),
    /// Zoom (delayed-sweep) window on/off.
    ZoomWin(bool),
    /// Timeline (deep) view on/off.
    Deep(bool),
    /// Timeline window duration, seconds.
    DeepSpan(f64),
    /// How the timeline tracks live acquisition.
    DeepFollow(crate::deep::Follow),
    Decode(crate::decode::Protocol),
    /// Decoder line assignment: line index -> channel.
    DecodeLine(usize, usize),
    /// UART baud rate.
    DecodeBaud(f64),
    Pan(crate::view::Pan),
    Home,
    Acq(AcqMode),
    /// Automatic peak detect at slow time bases on/off.
    AutoPeak(bool),
    Mode(TraceMode),
    Persist(Persistence),
    Gain(f32),
    Crt(bool),
    Select(usize),
    Guides(bool),
    Markers(bool),
    Record(bool),
    RecordClear,
    Export(String, String),
    PaletteSet(crate::gpu::Palette),
    WindowSize(f32, f32),
    UiScaleSet(f32),
    /// Scrollback memory budget, bytes.
    Scrollback(usize),
    SettingsOpen(bool),
    Math(Option<MathOp>),
    Run(bool),
    Multi(MultiMode),
    PfOut(bool),
    Cursor {
        amp: bool,
        on: bool,
    },
    Stats(usize),
    StatsReset,
    Fft(bool),
    FftSrc(usize),
    FftWnd(Window),
    Pf(bool),
    PfSrc(usize),
    PfTol(f64, f64),
    PfCapture,
    PfReset,
    Menu(Option<Menu>),
    /// Band plan / RF map verbs (`bandplan`, `bandmap`).
    RefMap(crate::refmap::RefMapAction),
    /// Export the UI element tree to a JSON file (`uitree <path>`).
    UiTree(String),
    MeasWin(bool),
    /// Open exactly these dock sections (`dock a,b|none`); `menu` opens one.
    Dock(Vec<Menu>),
    /// Window position, physical pixels (`windowpos X Y`).
    WindowPos(i32, i32),
    Layout(String),
    /// Whole-window capture (WYSIWYG, egui included) to `.png`/`.ppm`,
    /// optionally cropped to an ROI in captured-image pixels (`shot`).
    Shot {
        path: String,
        roi: Option<(u32, u32, u32, u32)>,
    },
    /// Raw plot-texture readback (`shotplot`); `roi` is plot pixels.
    ShotPlot {
        path: String,
        roi: Option<(u32, u32, u32, u32)>,
    },
    TrigPos(f64),
    HistoryIdx(usize),
    HistoryStep(i64),
    HistoryLive,
    CapSave(String),
    CapLoad(String),
    RefSave(usize),
    RefShow(bool),
    RefClear,
    Waterfall(bool),
    Viz(crate::viz::three_d::Viz3d),
    Effect(Option<String>),
    EffectReload,
    SessionSave(String),
    SessionLoad(String),
    Quit,
}
