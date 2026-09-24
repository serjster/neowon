//! Test/automation scripting. `NEOWON_SCRIPT=<path>` runs a plain-text
//! action script against the live app — the same state mutations the UI
//! performs, so anything the panel can do a script can do.
//!
//! One action per line (`#` comments allowed). Every UI control is
//! reachable here (AGENTS.md script-parity rule).
//!
//! ```text
//! wait <seconds>
//! stimulus <name>                       # sim scenarios, e.g. xy-circle
//! rate <S/s>
//! vdiv <ch> <volts>
//! enable <ch> <0|1>
//! coupling <ch> <dc|ac|gnd>
//! probe <ch> <factor>
//! offset <ch> <fraction>
//! trigger <ch> <rising|falling> <level_volts> <auto|normal|single>
//! trigpulse <ch> <pos|neg> <gt|eq|lt> <width_us> <auto|normal|single>
//! trigslope <ch> <pos|neg> <gt|eq|lt> <width_us> <upper_v> <lower_v> <sweep>
//! trigvideo <line|field|odd|even|linenum> <line#> <sweep>
//! holdoff <seconds>
//! autoset
//! force
//! timebase <s/div>                    # primary horizontal control
//! zoom <h|v> <in|out>                 # h = horizontal (see hzoom), v = V/div
//! hzoom <in|out>                      # time base, or zoom window when on
//! zoomwin <on|off>                    # zoom (delayed sweep) window
//! deep <on|off>                       # timeline view over the scrollback
//! deepspan <seconds>                  # timeline window duration
//! deepfollow <page|slide>             # how the timeline tracks live data
//! decode <off|uart|i2c|spi|onewire>    # protocol decoder
//! decodeline <line> <ch>               # assign a decoder input
//! decodebaud <baud>                    # UART baud rate
//! hview <centre> <span>               # zoom window, fractions of record
//! pan <left|right|up|down>            # window (h) / offset (v), one step
//! home                                # default zoom + centre position
//! acq <sample|peak|avg4|avg16|avg64>
//! autopeak <on|off>                    # auto peak detect at slow time bases
//! mode <vectors|dots|xy>
//! persist <off|inf|SECONDS>
//! gain <float>
//! math <off|add|sub|mul|div|diff|integ>
//! run <0|1>
//! multi <trigout|pfout|trigin>
//! pfout <0|1>
//! cursor <time|amp> <on|off>
//! stats <slot>
//! statsreset
//! fft <on|off>
//! fftsrc <slot>
//! fftwnd <rectangle|hamming|hann|blackman|flattop|triangular>
//! pf <on|off>
//! pfsrc <slot>
//! pftol <h_div> <v_div>
//! pfcapture
//! pfreset
//! menu <channel <ch>|horizontal|trigger|acquire|display|measure|math|cursor|utility|none>
//! bandplan <name> / bandmap <strip|mini|window> <on|off> / bandmap goto <band>
//! location <lat> <lon>|<locator>|ip|clear   # the operator's fix
//! refdb <fetch <source> [km]|import <source> <path>|clear <source>>
//! stations window|overlay <on|off> / radius <km|auto> / find <text|->
//! stations filter <source|service|mod> <value|-> / onair / scope / sort
//! stations tune|catalog <source:id>     # tune + demod / copy
//! uitree <path>                        # UI element tree (JSON), like a DOM
//! measwin <on|off>                     # measurements window
//! dock <section,section…|none>          # open exactly these dock sections
//! windowpos <x> <y>                     # window position, physical px
//! markers <0|1>                         # on-graph drag handles
//! record <0|1> / recordclear
//! export <wav|csv|raw> <path>           # write the recording
//! capsave <path.nwc> / capload <path>   # capture files (.nwc, vendor .cap)
//! history <idx|prev|next|live>          # scrub the recorded ring
//! refsave <ch> / ref <on|off> / refclear  # ghost reference traces
//! sessionsave <path> / sessionload <path> # setup files (are scripts)
//! trigpos <fraction>                    # horizontal trigger position
//! waterfall <on|off>                    # realtime spectrogram window
//! viz <off|terrain|tunnel|phase|xytime> # 3D signal viewport
//! palette <phosphor|thermal|green>
//! window <W>x<H>                        # resize (layout tests)
//! uiscale <factor>                      # egui zoom factor (hi-DPI screens)
//! scrollback <bytes>                    # capture history memory budget
//! settings <on|off>                     # Settings window
//! layout <path.json>                    # named-ROI map + open menu
//! shot <path> [x y w h]                 # whole window (WYSIWYG); x y w h crops it
//! shotplot <path> [x y w h]             # raw plot texture (1000x500), not the UI
//! quit
//! ```
//! `wait` advances a cumulative timeline; other actions fire when their
//! time comes. `quit` waits for outstanding shots, then exits.

//!
//! The vocabulary is `action.rs`, the text grammar `grammar.rs`, execution
//! `run.rs`, window/plot capture `shot.rs`; this file holds the queue and
//! the `NEOWON_SCRIPT` loader.

use std::collections::VecDeque;

use bevy::prelude::*;

mod action;
mod grammar;
mod run;
pub(crate) mod shot;

pub use action::Action;
pub(crate) use grammar::parse;
pub use run::run_script;

#[derive(Resource, Default)]
pub struct Script {
    /// (due time in seconds since startup, action)
    queue: VecDeque<(f64, Action)>,
}

impl Script {
    /// UI-injected action: due immediately, applied on the next
    /// `run_script` pass — buttons and scripts share one code path.
    pub fn inject(&mut self, action: Action) {
        self.queue.push_back((0.0, action));
    }

    /// Control-socket injection with an explicit due time (supports
    /// `wait` inside a remotely submitted script fragment).
    pub fn inject_at(&mut self, due: f64, action: Action) {
        self.queue.push_back((due, action));
    }
}

pub fn load_from_env() -> Script {
    let Some(path) = std::env::var_os("NEOWON_SCRIPT") else {
        return Script::default();
    };
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("NEOWON_SCRIPT {path:?}: {e}"));
    match parse(&text) {
        Ok(queue) => {
            info!("script: {} actions from {path:?}", queue.len());
            Script { queue }
        }
        Err(e) => panic!("NEOWON_SCRIPT parse error: {e}"),
    }
}
