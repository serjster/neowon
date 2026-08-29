//! Blocking VDS1022 device driver. Intended to be owned by a dedicated
//! acquisition thread; nothing here is async.

use std::path::Path;
use std::time::{Duration, Instant};

use nusb::descriptors::TransferType;
use nusb::transfer::{Bulk, Direction, In, Out, TransferError};
use nusb::{Device, Endpoint, Interface, MaybeFuture};
use tracing::{debug, info};

use neowon_backend::MultiMode;
use neowon_core::{AcqMode, CaptureFrame, ChannelCapture, Coupling, Slope, Sweep, TriggerKind};

use crate::consts::{self, ADC_CLIP, FLASH_SIZE, FRAME_SIZE, HTP_ERR, reg, status};
use crate::error::{Error, Result};
use crate::flash::FlashCal;
use crate::fpga;
use crate::frame::RawFrame;

/// Read-buffer size: covers 5-byte acks, 2002-byte flash, 5211-byte frames.
/// Multiple of 512 so high-speed IN transfers can't overflow.
const READ_BUF: usize = 6144;
const DEFAULT_TIMEOUT: Duration = Duration::from_millis(200);
const LONG_TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Copy)]
pub struct ChannelSetup {
    pub enabled: bool,
    /// Voltbase index 0..=9 (5 mV/div .. 5 V/div).
    pub vb: usize,
    pub coupling: Coupling,
    pub probe: f64,
    /// Vertical offset as a fraction of full scale, -0.5..=0.5.
    pub offset: f64,
}

impl Default for ChannelSetup {
    fn default() -> Self {
        Self {
            enabled: false,
            vb: 7,
            coupling: Coupling::Dc,
            probe: 1.0,
            offset: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Resp {
    status: u8,
    value: u32,
}

pub struct Vds1022 {
    _device: Device,
    _interface: Interface,
    ep_out: Endpoint<Bulk, Out>,
    ep_in: Endpoint<Bulk, In>,
    pub cal: FlashCal,
    /// True if this connection uploaded the FPGA bitstream (cold start).
    pub cold_start: bool,
    channels: [ChannelSetup; 2],
    sample_rate: f64,
    sweep: Sweep,
    trg_source: usize,
    roll: bool,
    peak: bool,
    seq: u64,
    last_io: Instant,
}

impl Vds1022 {
    /// Enumerate, claim, probe machine type, read calibration, and ensure the
    /// FPGA is loaded (uploading from `fpga_dir` if the device just powered
    /// on). Leaves the device initialized but stopped.
    pub fn open(fpga_dir: Option<&Path>) -> Result<Self> {
        let info = nusb::list_devices()
            .wait()?
            .find(|d| d.vendor_id() == consts::USB_VID && d.product_id() == consts::USB_PID)
            .ok_or(Error::NoDevice)?;
        debug!(
            bus = info.bus_id(),
            addr = info.device_address(),
            "found device"
        );
        let device = info.open().wait()?;
        let interface = device.claim_interface(0).wait()?;

        // Endpoint addresses are discovered, not assumed (the vendor apps do
        // the same).
        let (mut ep_out_addr, mut ep_in_addr) = (None, None);
        if let Some(desc) = interface.descriptor() {
            for ep in desc.endpoints() {
                if ep.transfer_type() == TransferType::Bulk {
                    match ep.direction() {
                        Direction::Out => ep_out_addr.get_or_insert(ep.address()),
                        Direction::In => ep_in_addr.get_or_insert(ep.address()),
                    };
                }
            }
        }
        let ep_out_addr =
            ep_out_addr.ok_or_else(|| Error::Protocol("no bulk OUT endpoint".into()))?;
        let ep_in_addr = ep_in_addr.ok_or_else(|| Error::Protocol("no bulk IN endpoint".into()))?;
        debug!(ep_out = ep_out_addr, ep_in = ep_in_addr, "bulk endpoints");

        let ep_out = interface.endpoint::<Bulk, Out>(ep_out_addr)?;
        let ep_in = interface.endpoint::<Bulk, In>(ep_in_addr)?;

        let mut dev = Self {
            _device: device,
            _interface: interface,
            ep_out,
            ep_in,
            cal: FlashCal {
                version: 0,
                gain: [[0; 10]; 2],
                ampl: [[0; 10]; 2],
                comp: [[0; 10]; 2],
                oem: 0,
                hw_version: String::new(),
                serial: String::new(),
                phasefine: 0,
            },
            cold_start: false,
            channels: [ChannelSetup::default(); 2],
            sample_rate: 0.0,
            sweep: Sweep::Auto,
            trg_source: 0,
            roll: false,
            peak: false,
            seq: 0,
            last_io: Instant::now(),
        };

        // Machine-type probe. The vendor app sleeps 50 ms between write and
        // read here (PortFilterTiny).
        dev.write_raw(
            &cmd_bytes(reg::MACHINE_TYPE, 1, b'V' as u32),
            DEFAULT_TIMEOUT,
        )?;
        std::thread::sleep(Duration::from_millis(50));
        let resp = dev.read_resp(Duration::from_secs(1))?;
        match resp.value {
            1 => {}
            other => return Err(Error::WrongMachine(other)),
        }

        // Factory calibration + identity from flash.
        let flash = dev.read_flash()?;
        dev.cal = FlashCal::parse(&flash)?;
        info!(
            serial = %dev.cal.serial,
            hw = %dev.cal.hw_version,
            phasefine = dev.cal.phasefine,
            "connected"
        );

        // FPGA: upload on cold start only.
        if !dev.query_fpga()? {
            let dir = fpga_dir.ok_or_else(|| {
                Error::Fpga("device needs an FPGA bitstream but no --fpga-dir given".into())
            })?;
            let generation = fpga::fpga_generation(&dev.cal.hw_version)?;
            let path = fpga::find_bitstream(dir, generation)?;
            info!(path = %path.display(), "uploading FPGA bitstream");
            dev.load_fpga_from(&path)?;
            dev.cold_start = true;
        }

        dev.init()?;
        Ok(dev)
    }

    // ---- low-level transport ----

    fn write_raw(&mut self, data: &[u8], timeout: Duration) -> Result<()> {
        let mut buf = self.ep_out.allocate(data.len());
        buf.extend_from_slice(data);
        let comp = self.ep_out.transfer_blocking(buf, timeout);
        self.last_io = Instant::now();
        match comp.status {
            Ok(()) if comp.actual_len == data.len() => Ok(()),
            Ok(()) => Err(Error::Protocol(format!(
                "short write: {} of {}",
                comp.actual_len,
                data.len()
            ))),
            Err(TransferError::Cancelled) => Err(Error::Timeout),
            Err(e) => Err(e.into()),
        }
    }

    fn read_raw(&mut self, timeout: Duration) -> Result<Vec<u8>> {
        let mut buf = self.ep_in.allocate(READ_BUF);
        buf.set_requested_len(READ_BUF);
        let comp = self.ep_in.transfer_blocking(buf, timeout);
        self.last_io = Instant::now();
        match comp.status {
            Ok(()) => {
                let n = comp.actual_len;
                let mut v = comp.buffer.into_vec();
                v.truncate(n);
                Ok(v)
            }
            Err(TransferError::Cancelled) => Err(Error::Timeout),
            Err(e) => Err(e.into()),
        }
    }

    fn read_resp(&mut self, timeout: Duration) -> Result<Resp> {
        let data = self.read_raw(timeout)?;
        parse_resp(&data)
    }

    /// Write a register and read the 5-byte acknowledgement.
    fn send(&mut self, addr: u32, width: u8, value: u32) -> Result<Resp> {
        self.write_raw(&cmd_bytes(addr, width, value), DEFAULT_TIMEOUT)?;
        self.read_resp(DEFAULT_TIMEOUT)
    }

    // ---- boot / identity ----

    fn read_flash(&mut self) -> Result<Vec<u8>> {
        self.write_raw(&cmd_bytes(reg::READ_FLASH, 1, 1), DEFAULT_TIMEOUT)?;
        let data = self.read_raw(LONG_TIMEOUT)?;
        if data.len() != FLASH_SIZE {
            return Err(Error::Flash(format!(
                "flash read returned {} bytes",
                data.len()
            )));
        }
        Ok(data)
    }

    pub fn query_fpga(&mut self) -> Result<bool> {
        Ok(self.send(reg::QUERY_FPGA, 1, 0)?.value == 1)
    }

    pub fn load_fpga_from(&mut self, path: &Path) -> Result<()> {
        let bits = std::fs::read(path)?;
        self.write_raw(
            &cmd_bytes(reg::LOAD_FPGA, 4, bits.len() as u32),
            DEFAULT_TIMEOUT,
        )?;
        let resp = self.read_resp(LONG_TIMEOUT)?;
        let frame_size = resp.value as usize;
        if frame_size <= 4 || frame_size > (1 << 20) {
            return Err(Error::Fpga(format!("implausible chunk size {frame_size}")));
        }
        let payload = frame_size - 4;
        let total = bits.len().div_ceil(payload);
        for (i, chunk) in bits.chunks(payload).enumerate() {
            let mut msg = Vec::with_capacity(4 + chunk.len());
            msg.extend_from_slice(&(i as u32).to_le_bytes());
            msg.extend_from_slice(chunk);
            self.write_raw(&msg, LONG_TIMEOUT)?;
            let ack = self.read_resp(LONG_TIMEOUT)?;
            if ack.status != status::OK || ack.value != i as u32 {
                return Err(Error::Fpga(format!(
                    "chunk {i}/{total} rejected: status {:#04x} value {}",
                    ack.status, ack.value
                )));
            }
            if i % 8 == 0 {
                debug!("fpga chunk {i}/{total}");
            }
        }
        std::thread::sleep(Duration::from_millis(100));
        info!(
            "FPGA bitstream loaded ({} bytes, {total} chunks)",
            bits.len()
        );
        Ok(())
    }

    // ---- register init & configuration ----

    /// The vendor init sequence: everything off, sane defaults, 250 kS/s.
    fn init(&mut self) -> Result<()> {
        self.send(reg::SET_CHL_ON, 1, 0)?;
        self.send(reg::SET_PHASEFINE, 2, self.cal.phasefine as u32)?;
        self.send(reg::SET_PEAKMODE, 1, 0)?;
        self.send(reg::SET_ROLLMODE, 1, 0)?;
        self.send(reg::SET_DEEPMEMORY, 2, 5100)?;
        self.send(reg::SET_PRE_TRG, 2, 5100)?;
        self.send(reg::SET_SUF_TRG, 4, 0)?;
        self.send(reg::SET_MULTI, 1, 0)?; // MULTI port = trigger out
        self.send(reg::SET_TRIGGER, 2, 0)?; // CH1 edge rise, auto
        for ch in 0..2 {
            self.send(holdoff_reg(ch), 2, 0x8002)?; // 100 ns
            self.send(edge_level_reg(ch), 2, 0x807F)?; // disabled (hi 127 / lo -128)
            self.send(freqref_reg(ch), 1, 20)?;
        }
        self.set_sample_rate(250e3)?;
        Ok(())
    }

    /// Configure one channel. Write order matters: zero offset does not clear
    /// the device sample buffer, gain and channel-config writes do.
    pub fn configure_channel(&mut self, ch: usize, setup: ChannelSetup) -> Result<()> {
        assert!(ch < 2 && setup.vb < 10);
        let relay = setup.vb >= consts::ATTENUATION_VB;
        let pos0 = (250.0 * setup.offset.clamp(-0.5, 0.5)).round() as i64;
        let ampl = self.cal.ampl[ch][setup.vb] as i64;
        let comp = self.cal.comp[ch][setup.vb] as i64;
        let zero = (comp - pos0 * ampl / 100).clamp(0, 4095) as u32;
        let gain = (self.cal.gain[ch][setup.vb] as i64).clamp(0, 4095) as u32;

        self.send(zero_off_reg(ch), 2, zero)?;
        self.send(volt_gain_reg(ch), 2, gain)?;

        let byte = (if setup.enabled { 0x80 } else { 0 })
            | (consts::coupling_code(setup.coupling) << 5)
            | (if relay { 0x02 } else { 0 });
        self.send(channel_reg(ch), 1, byte as u32)?;

        self.channels[ch] = setup;
        let mask = (self.channels[0].enabled as u32) | ((self.channels[1].enabled as u32) << 1);
        self.send(reg::SET_CHL_ON, 1, mask)?;
        Ok(())
    }

    /// Set the sampling rate (snapped to the prescaler ladder) and re-center
    /// the trigger position. Engages roll mode below 2.5 kS/s, as the
    /// hardware requires. Returns the actual rate.
    pub fn set_sample_rate(&mut self, rate: f64) -> Result<f64> {
        let ps = consts::prescaler_for_rate(rate);
        self.send(reg::SET_TIMEBASE, 4, ps)?;
        self.sample_rate = consts::CLOCK_HZ / ps as f64;
        let roll = self.sample_rate < consts::ROLLMODE_THRESHOLD;
        if roll != self.roll {
            self.roll = roll;
            self.send(reg::SET_ROLLMODE, 1, roll as u32)?;
        }
        self.set_trigger_position(0.5)?;
        Ok(self.sample_rate)
    }

    /// Hardware peak-detect: each sample pair becomes (max, min) over the
    /// decimation interval.
    pub fn set_peak_mode(&mut self, on: bool) -> Result<()> {
        if on != self.peak {
            self.peak = on;
            self.send(reg::SET_PEAKMODE, 1, on as u32)?;
        }
        Ok(())
    }

    /// Trigger holdoff. Encoding: 10-bit mantissa in units of 10 ns scaled by
    /// a decimal exponent, packed `(arg << 6) | exp` and byte-swapped.
    pub fn set_holdoff(&mut self, ch: usize, seconds: f64) -> Result<()> {
        self.send(holdoff_reg(ch), 2, holdoff_arg(seconds))
            .map(drop)
    }

    pub fn sample_rate(&self) -> f64 {
        self.sample_rate
    }

    /// Horizontal trigger position as a fraction of the record (0 = trigger
    /// at the far left, 0.5 = centered). Roll mode forces center.
    pub fn set_trigger_position(&mut self, position: f64) -> Result<()> {
        let position = if self.roll { 0.5 } else { position };
        let htp = (5000.0 * (0.5 - position).clamp(-0.5, 0.5)).round() as i32;
        let pre = (2550 - htp - HTP_ERR).max(0) as u32;
        let suf = (2550 + htp + HTP_ERR).max(0) as u32;
        self.send(reg::SET_PRE_TRG, 2, pre)?;
        self.send(reg::SET_SUF_TRG, 4, suf)?;
        self.send(reg::SET_EMPTY, 1, 1)?;
        Ok(())
    }

    /// Edge trigger from a channel source, single-trigger mode.
    pub fn set_edge_trigger(
        &mut self,
        ch: usize,
        slope: Slope,
        level_volts: f64,
        sweep: Sweep,
    ) -> Result<()> {
        self.set_trigger(ch, &TriggerKind::Edge { slope }, level_volts, sweep)
    }

    /// Trigger from a channel source, single-trigger mode. `level_volts` is
    /// the edge/pulse level; slope thresholds come from the kind itself.
    pub fn set_trigger(
        &mut self,
        ch: usize,
        kind: &TriggerKind,
        level_volts: f64,
        sweep: Sweep,
    ) -> Result<()> {
        assert!(ch < 2);
        let setup = self.channels[ch];
        let range = consts::full_scale_volts(setup.vb) * setup.probe;
        let to_raw = |volts: f64| {
            ((volts / range + setup.offset) * 250.0)
                .round()
                .clamp(-128.0, 127.0) as i32
        };

        self.send(reg::SET_TRIGGER, 2, trigger_word(ch, kind, sweep) as u32)?;
        self.sweep = sweep;
        self.trg_source = ch;

        match kind {
            TriggerKind::Edge { slope } => {
                self.write_level_pair(ch, to_raw(level_volts), *slope)?;
            }
            TriggerKind::Pulse { condition, width } => {
                // Pulse level lives in the edge level registers. The level
                // pair's polarity must follow the pulse polarity: a negative
                // pulse arms on the falling excursion (hardware-verified —
                // rising-style packing makes negative conditions starve).
                let slope = match condition.code() {
                    0..=2 => Slope::Rising,
                    _ => Slope::Falling,
                };
                self.write_level_pair(ch, to_raw(level_volts), slope)?;
                let (gl, hl) = width_split(*width);
                self.send(width_gl_reg(ch), 2, gl as u32)?;
                self.send(width_hl_reg(ch), 2, hl as u32)?;
            }
            TriggerKind::Slope {
                width,
                upper,
                lower,
                ..
            } => {
                let (mut up, mut lo) = (to_raw(*upper), to_raw(*lower));
                if up < lo {
                    (up, lo) = (lo, up); // upper must exceed lower
                }
                self.send(slope_thred_reg(ch), 2, slope_thred_word(up, lo))?;
                // Frequency-meter reference sits at the window midpoint.
                self.send(freqref_reg(ch), 1, (((up + lo) / 2) as i8 as u8) as u32)?;
                let (gl, hl) = width_split(*width);
                self.send(width_gl_reg(ch), 2, gl as u32)?;
                self.send(width_hl_reg(ch), 2, hl as u32)?;
            }
            TriggerKind::Video { line, .. } => {
                self.send(reg::SET_VIDEOLINE, 2, *line as u32)?;
            }
        }
        Ok(())
    }

    /// Edge/pulse level pair with 10-LSB hysteresis, plus the
    /// frequency-meter reference just below the level.
    fn write_level_pair(&mut self, ch: usize, level: i32, slope: Slope) -> Result<()> {
        let (hi, lo) = match slope {
            Slope::Rising => {
                let hi = level.min(127);
                (hi, (hi - 10).max(-128))
            }
            Slope::Falling => {
                let lo = level.max(-128);
                ((lo + 10).min(127), lo)
            }
        };
        let val = ((hi as i8 as u8) as u32) | (((lo as i8 as u8) as u32) << 8);
        self.send(edge_level_reg(ch), 2, val)?;
        self.send(freqref_reg(ch), 1, ((level - 5) as i8 as u8) as u32)?;
        Ok(())
    }

    /// MULTI port function: 0 = trigger out, 1 = pass/fail out, 2 = trigger in.
    pub fn set_multi(&mut self, mode: MultiMode) -> Result<()> {
        let code = match mode {
            MultiMode::TriggerOut => 0,
            MultiMode::PassFailOut => 1,
            MultiMode::TriggerIn => 2,
        };
        self.send(reg::SET_MULTI, 1, code).map(drop)
    }

    /// Pass/fail TTL level on the MULTI port.
    pub fn set_pf_level(&mut self, high: bool) -> Result<()> {
        self.send(reg::SET_PF, 1, high as u32).map(drop)
    }

    pub fn run(&mut self) -> Result<()> {
        self.send(reg::SET_RUNSTOP, 1, 0).map(drop)
    }

    pub fn stop(&mut self) -> Result<()> {
        self.send(reg::SET_RUNSTOP, 1, 1).map(drop)
    }

    pub fn force_trigger(&mut self) -> Result<()> {
        self.send(reg::SET_FORCETRG, 1, 3).map(drop)
    }

    pub fn triggered_mask(&mut self) -> Result<u32> {
        Ok(self.send(reg::GET_TRIGGERED, 1, 0)?.value)
    }

    pub fn data_finished(&mut self) -> Result<bool> {
        Ok(self.send(reg::GET_DATAFINISHED, 1, 1)?.value != 0)
    }

    /// The link drops if the device sees no traffic for ~3 s; callers with
    /// idle periods should invoke this periodically.
    pub fn keep_alive(&mut self) -> Result<()> {
        if self.last_io.elapsed() > Duration::from_secs(2) {
            self.stop()?;
        }
        Ok(())
    }

    // ---- acquisition ----

    /// Request one record. Returns `Error::NotReady` when the device has no
    /// data yet (caller retries after ~60 ms, per the vendor apps).
    pub fn get_frames(&mut self) -> Result<Vec<RawFrame>> {
        let on = [self.channels[0].enabled, self.channels[1].enabled];
        let n = on.iter().filter(|&&e| e).count();
        if n == 0 {
            return Err(Error::Protocol("no channel enabled".into()));
        }
        // Normal/Single sweeps gate on the hardware: a completed record AND a
        // real trigger event on the source channel. Auto just pulls.
        if self.sweep != Sweep::Auto && !self.roll {
            if !self.data_finished()? {
                return Err(Error::NotReady);
            }
            let triggered = self.triggered_mask()?;
            if triggered & (1 << self.trg_source as u32) == 0 {
                return Err(Error::NotReady);
            }
        }
        // byte0 = CH1 state, byte1 = CH2 state; 0x05 = on, 0x04 = off.
        let value = (state_code(on[0]) as u32) | ((state_code(on[1]) as u32) << 8);
        self.write_raw(&cmd_bytes(reg::GET_DATA, 2, value), DEFAULT_TIMEOUT)?;
        let mut frames = Vec::with_capacity(n);
        for _ in 0..n {
            let data = self.read_raw(DEFAULT_TIMEOUT)?;
            if data.len() == 5 {
                let resp = parse_resp(&data)?;
                if resp.status == status::EMPTY {
                    return Err(Error::NotReady);
                }
                return Err(Error::Protocol(format!(
                    "unexpected 5-byte reply to GET_DATA: status {:#04x}",
                    resp.status
                )));
            }
            if data.len() != FRAME_SIZE {
                return Err(Error::Protocol(format!("frame of {} bytes", data.len())));
            }
            frames.push(RawFrame::parse(&data)?);
        }
        Ok(frames)
    }

    /// Non-blocking capture: `Ok(None)` when the device has no data yet.
    pub fn try_capture(&mut self) -> Result<Option<CaptureFrame>> {
        match self.get_frames() {
            Ok(frames) => Ok(Some(self.assemble_capture(frames))),
            Err(Error::NotReady) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Poll `get_frames` until data arrives or `deadline` passes, then
    /// convert to a calibrated `CaptureFrame`.
    pub fn capture(&mut self, max_wait: Duration) -> Result<CaptureFrame> {
        let deadline = Instant::now() + max_wait;
        loop {
            match self.get_frames() {
                Ok(frames) => return Ok(self.assemble_capture(frames)),
                Err(Error::NotReady) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(60));
                }
                Err(e) => return Err(e),
            }
        }
    }

    fn assemble_capture(&mut self, frames: Vec<RawFrame>) -> CaptureFrame {
        self.seq += 1;
        let channels = frames
            .into_iter()
            .map(|f| {
                let ch = f.channel as usize;
                let setup = self.channels[ch.min(1)];
                let range = consts::full_scale_volts(setup.vb) * setup.probe;
                ChannelCapture {
                    ch,
                    clipped: f.clipped(),
                    freq_meter: f.freq_meter(),
                    raw: f.samples().to_vec(),
                    volts_per_lsb: range / consts::ADC_RANGE,
                    zero_volts: -setup.offset * range,
                }
            })
            .collect();
        CaptureFrame {
            seq: self.seq,
            sample_rate: self.sample_rate,
            acq: if self.peak {
                AcqMode::Peak
            } else {
                AcqMode::Sample
            },
            channels,
        }
    }

    pub fn channel(&self, ch: usize) -> ChannelSetup {
        self.channels[ch]
    }
}

fn state_code(on: bool) -> u8 {
    if on { 0x05 } else { 0x04 }
}

fn channel_reg(ch: usize) -> u32 {
    // Note CH2-before-CH1 ordering in the register file.
    [reg::SET_CHANNEL_CH1, reg::SET_CHANNEL_CH2][ch]
}
fn zero_off_reg(ch: usize) -> u32 {
    [reg::SET_ZERO_OFF_CH1, reg::SET_ZERO_OFF_CH2][ch]
}
fn volt_gain_reg(ch: usize) -> u32 {
    [reg::SET_VOLT_GAIN_CH1, reg::SET_VOLT_GAIN_CH2][ch]
}
fn edge_level_reg(ch: usize) -> u32 {
    [reg::SET_EDGE_LEVEL_CH1, reg::SET_EDGE_LEVEL_CH2][ch]
}
fn holdoff_reg(ch: usize) -> u32 {
    [reg::SET_TRG_HOLDOFF_CH1, reg::SET_TRG_HOLDOFF_CH2][ch]
}
fn freqref_reg(ch: usize) -> u32 {
    [reg::SET_FREQREF_CH1, reg::SET_FREQREF_CH2][ch]
}
fn slope_thred_reg(ch: usize) -> u32 {
    [reg::SET_SLOPE_THRED_CH1, reg::SET_SLOPE_THRED_CH2][ch]
}
fn width_gl_reg(ch: usize) -> u32 {
    [reg::SET_TRG_WIDTH_GL_CH1, reg::SET_TRG_WIDTH_GL_CH2][ch]
}
fn width_hl_reg(ch: usize) -> u32 {
    [reg::SET_TRG_WIDTH_HL_CH1, reg::SET_TRG_WIDTH_HL_CH2][ch]
}

/// Trigger word (`SET_TRIGGER`, single-trigger mode): bit 0 = external
/// source (never for us), bit 8 = type-code bit 0, bit 14 = type-code bit 1,
/// bit 13 = source channel. Type codes per the Java app: Edge=0, Slope=1,
/// Video=2, Pulse=3 (vds1022.py has Slope/Video swapped — not copied).
fn trigger_word(ch: usize, kind: &TriggerKind, sweep: Sweep) -> u16 {
    fn type_bits(code: u16) -> u16 {
        ((code & 1) << 8) | (((code >> 1) & 1) << 14)
    }
    let sweep_code: u16 = match sweep {
        Sweep::Auto => 0,
        Sweep::Normal => 1,
        Sweep::Single => 2,
    };
    let mut word: u16 = 0;
    if ch == 1 {
        word |= 1 << 13;
    }
    match kind {
        TriggerKind::Edge { slope } => {
            word |= type_bits(consts::trg_type::EDGE);
            if *slope == Slope::Falling {
                word |= 1 << 12;
            }
            word |= sweep_code << 10; // bit 9 stays 0
        }
        TriggerKind::Pulse { condition, .. } => {
            word |= type_bits(consts::trg_type::PULSE);
            word |= condition.code() << 5;
            word |= sweep_code << 10;
        }
        TriggerKind::Slope { condition, .. } => {
            word |= type_bits(consts::trg_type::SLOPE);
            word |= condition.code() << 5;
            word |= sweep_code << 10;
        }
        // Hardware-unverified: the NTSC/PAL and module bits are not fully
        // decoded; only the sync mode is packed.
        TriggerKind::Video { sync, .. } => {
            word |= type_bits(consts::trg_type::VIDEO);
            word |= sync.code() << 10;
        }
    }
    word
}

/// Pulse/slope width for FPGA >= V3: `m = round(seconds * 1e8)` (units of
/// 10 ns), split into the low/high u16 register pair.
fn width_split(seconds: f64) -> (u16, u16) {
    let m = (seconds.max(0.0) * 1e8).round() as u64;
    ((m & 0xFFFF) as u16, (m >> 16) as u16)
}

/// Slope threshold pair packing: `(upper & 0xFF) | ((lower & 0xFF) << 8)`
/// with the two levels as raw i8 counts (upper > lower).
fn slope_thred_word(upper: i32, lower: i32) -> u32 {
    ((upper as i8 as u8) as u32) | (((lower as i8 as u8) as u32) << 8)
}

/// Holdoff wire encoding: `d = seconds * 1e8` (units of 10 ns), divided by 10
/// until it fits 10 bits, exponent in the low 3 bits, then byte-swapped.
fn holdoff_arg(seconds: f64) -> u32 {
    let mut d = (seconds * 1e8).max(1.0);
    let mut e = 0u32;
    while d > 1023.0 {
        d /= 10.0;
        e += 1;
    }
    let v = ((d.round() as u32) << 6) | (e & 7);
    ((v >> 8) & 0xFF) | ((v & 0xFF) << 8)
}

/// Wire format: u32 LE address, u8 value width (1/2/4), LE value bytes.
fn cmd_bytes(addr: u32, width: u8, value: u32) -> Vec<u8> {
    debug_assert!(matches!(width, 1 | 2 | 4));
    let mut b = Vec::with_capacity(5 + width as usize);
    b.extend_from_slice(&addr.to_le_bytes());
    b.push(width);
    b.extend_from_slice(&value.to_le_bytes()[..width as usize]);
    b
}

fn parse_resp(data: &[u8]) -> Result<Resp> {
    if data.len() != 5 {
        return Err(Error::Protocol(format!(
            "expected 5-byte response, got {} bytes",
            data.len()
        )));
    }
    Ok(Resp {
        status: data[0],
        value: u32::from_le_bytes(data[1..5].try_into().unwrap()),
    })
}

// Suppress unused warning: ADC_CLIP is used through RawFrame::clipped.
const _: i8 = ADC_CLIP;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_packing() {
        // SET_TIMEBASE (0x52), 4 bytes, prescaler 400
        assert_eq!(
            cmd_bytes(0x52, 4, 400),
            vec![0x52, 0, 0, 0, 4, 0x90, 0x01, 0, 0]
        );
        // MACHINE_TYPE probe: 0x4001, 1 byte, 'V'
        assert_eq!(cmd_bytes(0x4001, 1, 86), vec![0x01, 0x40, 0, 0, 1, 86]);
    }

    #[test]
    fn holdoff_encoding() {
        // 100 ns -> mantissa 10, exp 0 -> 0x0280 -> swapped 0x8002 (the
        // documented power-on default).
        assert_eq!(holdoff_arg(100e-9), 0x8002);
        // 1 ms -> 1e5 units of 10 ns -> mantissa 1000, exp 2
        let v = (1000u32 << 6) | 2;
        assert_eq!(holdoff_arg(1e-3), ((v >> 8) & 0xFF) | ((v & 0xFF) << 8));
    }

    #[test]
    fn response_parsing() {
        let r = parse_resp(&[b'S', 0x02, 0x20, 0, 0]).unwrap();
        assert_eq!(r.status, b'S');
        assert_eq!(r.value, 0x2002);
        assert!(parse_resp(&[1, 2, 3]).is_err());
    }

    use neowon_core::{PulseCondition, VideoSync};

    #[test]
    fn trigger_word_edge() {
        // CH1, rising, auto: the power-on default word.
        let w = trigger_word(
            0,
            &TriggerKind::Edge {
                slope: Slope::Rising,
            },
            Sweep::Auto,
        );
        assert_eq!(w, 0);
        // CH2, falling, single: channel + slope + sweep bits.
        let w = trigger_word(
            1,
            &TriggerKind::Edge {
                slope: Slope::Falling,
            },
            Sweep::Single,
        );
        assert_eq!(w, (1 << 13) | (1 << 12) | (2 << 10));
    }

    #[test]
    fn trigger_word_pulse() {
        // Pulse >400 us on CH2, normal sweep. Type code 3 -> bits 8 and 14.
        let kind = TriggerKind::Pulse {
            condition: PulseCondition::PositiveGreater,
            width: 400e-6,
        };
        let w = trigger_word(1, &kind, Sweep::Normal);
        assert_eq!(w, (1 << 14) | (1 << 13) | (1 << 10) | (1 << 8));
    }

    #[test]
    fn trigger_word_slope() {
        // Slope, negative-less (code 6: bit-2 polarity + comparator 2) on
        // CH2, single. Type code 1 -> bit 8.
        let kind = TriggerKind::Slope {
            condition: PulseCondition::NegativeLess,
            width: 10e-6,
            upper: 1.0,
            lower: -1.0,
        };
        let w = trigger_word(1, &kind, Sweep::Single);
        assert_eq!(w, (1 << 13) | (2 << 10) | (1 << 8) | (6 << 5));
    }

    #[test]
    fn trigger_word_video() {
        // Video, even-field (code 3) on CH1. Type code 2 -> bit 14.
        let kind = TriggerKind::Video {
            sync: VideoSync::EvenField,
            line: 0,
        };
        let w = trigger_word(0, &kind, Sweep::Auto);
        assert_eq!(w, (1 << 14) | (3 << 10));
    }

    #[test]
    fn width_splitting() {
        // 1 ms = 1e5 units of 10 ns = 0x186A0 -> gl 0x86A0, hl 0x0001.
        assert_eq!(width_split(1e-3), (0x86A0, 0x0001));
        // 400 us = 4e4 = 0x9C40 fits in the low half.
        assert_eq!(width_split(400e-6), (0x9C40, 0x0000));
        assert_eq!(width_split(0.0), (0, 0));
    }

    #[test]
    fn slope_thred_packing() {
        // upper 50, lower -30 -> 0x32 | (0xE2 << 8).
        assert_eq!(slope_thred_word(50, -30), 0x32 | (0xE2 << 8));
    }
}
