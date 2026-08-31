//! The acquisition timeline: many captured segments placed on a real time
//! axis and reduced to one min/max pair per display column.
//!
//! This is the model that stops the display assuming "one contiguous record
//! at a single sample rate", which is a property of one instrument rather
//! than of oscilloscopes. A triggered instrument with a small buffer hands
//! over short records separated by dead time it was not acquiring in; a
//! streaming source (a sound card, an SDR) hands over back-to-back chunks
//! with no dead time at all. Both are a list of segments with start times;
//! the second simply never produces a gap.
//!
//! The axis stays **proportional to real time**. Butting segments together
//! would look tidier, but every Δt, period and frequency reading spanning a
//! join would then be short by the elapsed dead time — on an instrument
//! whose job is measuring time, that is not a trade worth making. Columns
//! nothing covers are marked, and the caller draws them as gaps.

/// Column value meaning "no acquired data here".
///
/// `-128` is outside the sample encoding's usable range (±125 is full
/// scale) but *is* producible — the averager clamps to it — so reduced
/// values are clamped to ±127 and this code is reserved.
pub const NO_DATA: i8 = i8::MIN;

/// A fixed-tile min/max summary of one record, computed once when the
/// record is stored so that redrawing a long window does not have to touch
/// every sample. This is the standard audio peak-file trick.
#[derive(Debug, Clone)]
pub struct Tiles {
    pub tile: usize,
    pub min: Vec<i8>,
    pub max: Vec<i8>,
}

/// Summarize `raw` into `tile`-sample buckets.
pub fn summarize(raw: &[i8], tile: usize) -> Tiles {
    let tile = tile.max(1);
    let n = raw.len().div_ceil(tile);
    let (mut min, mut max) = (Vec::with_capacity(n), Vec::with_capacity(n));
    for chunk in raw.chunks(tile) {
        min.push(*chunk.iter().min().unwrap_or(&0));
        max.push(*chunk.iter().max().unwrap_or(&0));
    }
    Tiles { tile, min, max }
}

/// One acquired record placed on the session time axis.
#[derive(Debug, Clone, Copy)]
pub struct Segment<'a> {
    /// Time of `raw[0]`, seconds on the session clock.
    pub t0: f64,
    pub sample_rate: f64,
    pub raw: &'a [i8],
    /// Optional precomputed summary, used when a column spans many samples.
    pub tiles: Option<&'a Tiles>,
}

impl Segment<'_> {
    pub fn t_end(&self) -> f64 {
        self.t0 + self.raw.len() as f64 / self.sample_rate.max(1e-12)
    }
}

/// The reduced trace for one channel over one window.
#[derive(Debug, Clone, PartialEq)]
pub struct Reduced {
    pub columns: usize,
    /// `2 * columns` values: (min, max) per column, `NO_DATA` where nothing
    /// was acquired. Rendered as a vertical span per column, which is how a
    /// scope's envelope looks.
    pub pairs: Vec<i8>,
    /// Fraction of the window actually covered by acquisition, 0..=1.
    pub coverage: f64,
    /// Columns with no data, ascending — the mask the renderer marks.
    pub gaps: Vec<u32>,
}

impl Reduced {
    /// Number of *discontinuities* — runs of adjacent empty columns — not
    /// the number of empty columns. The distinction matters when reporting:
    /// widening the window puts more dead time on screen but gives each
    /// interval fewer columns, so a column count falls while the number of
    /// breaks in the signal rises.
    pub fn discontinuities(&self) -> usize {
        self.gaps.windows(2).filter(|w| w[1] != w[0] + 1).count()
            + usize::from(!self.gaps.is_empty())
    }
}

/// Measure of the union of `spans` intersected with `window`, as a fraction
/// of the window.
///
/// A union, not a sum: capture timestamps are estimates, so segments can
/// overlap slightly, and summing their durations would over-count and can
/// exceed 1. `spans` is sorted in place.
pub fn coverage(spans: &mut [(f64, f64)], window: (f64, f64)) -> f64 {
    let (w0, w1) = window;
    let width = w1 - w0;
    if width <= 0.0 {
        return 0.0;
    }
    spans.sort_by(|a, b| a.0.total_cmp(&b.0));
    let (mut total, mut cur): (f64, Option<(f64, f64)>) = (0.0, None);
    for &(s, e) in spans.iter() {
        let s = s.max(w0);
        let e = e.min(w1);
        if e <= s {
            continue;
        }
        match cur {
            Some((cs, ce)) if s <= ce => cur = Some((cs, ce.max(e))),
            Some((cs, ce)) => {
                total += ce - cs;
                cur = Some((s, e));
            }
            None => cur = Some((s, e)),
        }
    }
    if let Some((cs, ce)) = cur {
        total += ce - cs;
    }
    (total / width).clamp(0.0, 1.0)
}

/// Reduce with the dead time removed: segments are laid end to end and each
/// join is marked with a single column, so the signal gets the whole width.
///
/// The cost is that the x axis is no longer time — a Δt spanning a join is
/// short by however long the instrument was not acquiring — so callers must
/// label the axis as non-linear and suppress time readouts across joins.
pub fn reduce_collapsed(segments: &[Segment<'_>], window: (f64, f64), columns: usize) -> Reduced {
    let columns = columns.max(1);
    let mut inside: Vec<&Segment<'_>> = segments
        .iter()
        .filter(|s| !s.raw.is_empty() && s.t_end() > window.0 && s.t0 < window.1)
        .collect();
    inside.sort_by(|a, b| a.t0.total_cmp(&b.t0));
    if inside.is_empty() {
        return Reduced {
            columns,
            pairs: vec![NO_DATA; columns * 2],
            coverage: 0.0,
            gaps: (0..columns as u32).collect(),
        };
    }
    // One column per join, the rest shared out by each segment's duration.
    let joins = inside.len().saturating_sub(1);
    let usable = columns.saturating_sub(joins).max(1);
    let total: f64 = inside
        .iter()
        .map(|s| s.t_end().min(window.1) - s.t0.max(window.0))
        .sum();
    let mut pairs = vec![NO_DATA; columns * 2];
    let mut gaps = Vec::new();
    let mut col = 0usize;
    for (i, seg) in inside.iter().enumerate() {
        let (a, b) = (seg.t0.max(window.0), seg.t_end().min(window.1));
        let width = (((b - a) / total.max(1e-12)) * usable as f64).round() as usize;
        let width = width.max(1).min(columns.saturating_sub(col));
        if width == 0 {
            break;
        }
        let one = reduce(std::slice::from_ref(*seg), (a, b), width);
        pairs[col * 2..(col + width) * 2].copy_from_slice(&one.pairs);
        col += width;
        if i < joins && col < columns {
            gaps.push(col as u32);
            col += 1;
        }
    }
    Reduced {
        columns,
        pairs,
        coverage: 1.0,
        gaps,
    }
}

/// Reduce `segments` over `window` into `columns` min/max pairs.
///
/// `segments` may be in any order and may overlap. Columns are laid out
/// proportionally in time, so a gap occupies exactly the screen width its
/// duration deserves.
pub fn reduce(segments: &[Segment<'_>], window: (f64, f64), columns: usize) -> Reduced {
    let columns = columns.max(1);
    let (w0, w1) = window;
    let width = (w1 - w0).max(1e-12);
    let col_dt = width / columns as f64;
    let mut pairs = vec![NO_DATA; columns * 2];

    for seg in segments {
        if seg.raw.is_empty() || seg.t_end() <= w0 || seg.t0 >= w1 {
            continue;
        }
        // Columns this segment can touch.
        let c0 = (((seg.t0 - w0) / col_dt).floor().max(0.0)) as usize;
        let c1 = (((seg.t_end() - w0) / col_dt).ceil()).clamp(0.0, columns as f64) as usize;
        for col in c0..c1 {
            let (t_lo, t_hi) = (w0 + col as f64 * col_dt, w0 + (col + 1) as f64 * col_dt);
            // Sample range of this column inside the segment.
            let i0 = ((t_lo - seg.t0) * seg.sample_rate).floor();
            let i1 = ((t_hi - seg.t0) * seg.sample_rate).ceil();
            let i0 = i0.max(0.0) as usize;
            let i1 = (i1.max(0.0) as usize).min(seg.raw.len());
            if i1 <= i0 {
                continue;
            }
            let Some((lo, hi)) = extremes(seg, i0, i1) else {
                continue;
            };
            let (p_lo, p_hi) = (&mut pairs[col * 2], lo);
            if *p_lo == NO_DATA || p_hi < *p_lo {
                *p_lo = clamp_sample(p_hi);
            }
            let slot = &mut pairs[col * 2 + 1];
            if *slot == NO_DATA || hi > *slot {
                *slot = clamp_sample(hi);
            }
        }
    }

    let gaps: Vec<u32> = (0..columns)
        .filter(|&c| pairs[c * 2] == NO_DATA)
        .map(|c| c as u32)
        .collect();
    let mut spans: Vec<(f64, f64)> = segments
        .iter()
        .filter(|s| !s.raw.is_empty())
        .map(|s| (s.t0, s.t_end()))
        .collect();
    Reduced {
        columns,
        pairs,
        coverage: coverage(&mut spans, window),
        gaps,
    }
}

/// A reduced value can never be the sentinel.
fn clamp_sample(v: i8) -> i8 {
    v.max(-127)
}

/// Min and max of `raw[i0..i1]`, via the tile summary when the range is wide
/// enough for it to be exact-enough and much cheaper.
fn extremes(seg: &Segment<'_>, i0: usize, i1: usize) -> Option<(i8, i8)> {
    if let Some(t) = seg.tiles
        && i1 - i0 >= t.tile * 2
    {
        // Whole tiles strictly inside the range, plus the ragged ends read
        // from the samples, so the result matches the exact answer.
        let first = i0.div_ceil(t.tile);
        let last = i1 / t.tile;
        if first < last {
            let mut lo = *t.min[first..last].iter().min()?;
            let mut hi = *t.max[first..last].iter().max()?;
            for &v in &seg.raw[i0..(first * t.tile).min(i1)] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            for &v in &seg.raw[(last * t.tile).max(i0)..i1] {
                lo = lo.min(v);
                hi = hi.max(v);
            }
            return Some((lo, hi));
        }
    }
    let slice = seg.raw.get(i0..i1)?;
    Some((*slice.iter().min()?, *slice.iter().max()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seg<'a>(t0: f64, rate: f64, raw: &'a [i8]) -> Segment<'a> {
        Segment {
            t0,
            sample_rate: rate,
            raw,
            tiles: None,
        }
    }

    #[test]
    fn no_sample_is_lost_from_the_envelope() {
        // The property that matters for a scope display: every acquired
        // sample lies inside the drawn envelope. Which column owns a sample
        // exactly on a boundary is ambiguous to a float rounding, so the
        // check allows a one-column straddle.
        let raw: Vec<i8> = (0..1000).map(|i| ((i * 7) % 200 - 100) as i8).collect();
        let (rate, window, cols) = (1000.0, (0.0, 1.0), 100usize);
        let r = reduce(&[seg(0.0, rate, &raw)], window, cols);
        assert!(r.gaps.is_empty(), "a contiguous record has no gaps");
        assert!((r.coverage - 1.0).abs() < 1e-9);

        let dt = (window.1 - window.0) / cols as f64;
        for (i, &v) in raw.iter().enumerate() {
            let t = i as f64 / rate;
            let c = ((t - window.0) / dt) as usize;
            let lo = c.saturating_sub(1);
            let hi = (c + 2).min(cols);
            let mut covered = false;
            for col in lo..hi {
                if r.pairs[col * 2] != NO_DATA && r.pairs[col * 2] <= v && v <= r.pairs[col * 2 + 1]
                {
                    covered = true;
                }
            }
            assert!(covered, "sample {i} (value {v}) fell outside the envelope");
        }
    }

    #[test]
    fn an_empty_window_is_all_gap() {
        let r = reduce(&[], (0.0, 1.0), 16);
        assert!(r.pairs.iter().all(|&v| v == NO_DATA));
        assert_eq!(r.coverage, 0.0);
        assert_eq!(r.gaps.len(), 16);
    }

    #[test]
    fn dead_time_between_segments_becomes_gap_columns() {
        // Two 0.25 s records with 0.25 s of dead time between them, in a 1 s
        // window: half covered, and the gap lands where the clock says.
        let raw = [50i8; 250];
        let segs = [seg(0.0, 1000.0, &raw), seg(0.5, 1000.0, &raw)];
        let r = reduce(&segs, (0.0, 1.0), 100);
        assert!((r.coverage - 0.5).abs() < 1e-9, "coverage {}", r.coverage);
        // Columns 25..50 are the dead time.
        assert!(r.gaps.contains(&30));
        assert!(r.gaps.contains(&49));
        assert!(!r.gaps.contains(&10));
        assert!(!r.gaps.contains(&60));
    }

    #[test]
    fn overlapping_segments_never_exceed_full_coverage() {
        // Timestamps are estimates, so segments can overlap; summing their
        // durations would report 200 % covered.
        let raw = [10i8; 100];
        let segs = [seg(0.0, 100.0, &raw), seg(0.5, 100.0, &raw)];
        let r = reduce(&segs, (0.0, 1.5), 32);
        assert!(r.coverage <= 1.0);
        assert!((r.coverage - 1.0).abs() < 1e-9, "coverage {}", r.coverage);
    }

    #[test]
    fn segments_outside_the_window_are_ignored_safely() {
        let raw = [1i8; 100];
        // Entirely before, entirely after, and straddling each edge.
        let segs = [
            seg(-10.0, 100.0, &raw),
            seg(50.0, 100.0, &raw),
            seg(-0.5, 100.0, &raw),
            seg(0.5, 100.0, &raw),
        ];
        for cols in [1usize, 7, 100, 1000] {
            let r = reduce(&segs, (0.0, 1.0), cols);
            assert_eq!(r.pairs.len(), cols * 2);
            assert!((0.0..=1.0).contains(&r.coverage));
        }
    }

    #[test]
    fn the_sentinel_is_never_produced_by_real_data() {
        // -128 is producible upstream (the averager clamps to it) and must
        // not be mistaken for a gap.
        let raw = [i8::MIN; 100];
        let r = reduce(&[seg(0.0, 100.0, &raw)], (0.0, 1.0), 10);
        assert!(r.gaps.is_empty(), "clamped data read as a gap");
        assert!(r.pairs.iter().all(|&v| v == -127));
    }

    #[test]
    fn tiles_agree_with_the_raw_reduction() {
        let raw: Vec<i8> = (0..4096).map(|i| ((i * 13) % 250 - 125) as i8).collect();
        let t = summarize(&raw, 64);
        let window = (0.0, 1.0);
        let plain = reduce(&[seg(0.0, 4096.0, &raw)], window, 64);
        let tiled = reduce(
            &[Segment {
                t0: 0.0,
                sample_rate: 4096.0,
                raw: &raw,
                tiles: Some(&t),
            }],
            window,
            64,
        );
        assert_eq!(plain.pairs, tiled.pairs);
    }

    #[test]
    fn coverage_is_a_union_measure() {
        let mut spans = vec![(0.0, 0.5), (0.25, 0.75), (0.9, 1.2)];
        // Union inside [0,1] is 0.75 + 0.1 = 0.85.
        assert!((coverage(&mut spans, (0.0, 1.0)) - 0.85).abs() < 1e-9);
        let mut none: Vec<(f64, f64)> = vec![];
        assert_eq!(coverage(&mut none, (0.0, 1.0)), 0.0);
    }

    #[test]
    fn discontinuities_count_breaks_not_blank_columns() {
        let raw = [50i8; 100];
        // Three separate 0.1 s records in a 1 s window: two dead intervals
        // between them and one trailing, however many columns those happen
        // to occupy.
        let segs = [
            seg(0.0, 1000.0, &raw),
            seg(0.4, 1000.0, &raw),
            seg(0.8, 1000.0, &raw),
        ];
        for cols in [200usize, 1000] {
            let r = reduce(&segs, (0.0, 1.0), cols);
            assert_eq!(
                r.discontinuities(),
                3,
                "{cols} columns: {} gap columns",
                r.gaps.len()
            );
        }
    }
}
