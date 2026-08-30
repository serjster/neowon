# Phase 7.9: timeline, streaming backend, decoders

User direction, 2026-08-30: do the three things recommended after the FPGA
question — the deep view "reframed as an acquisition timeline", a streaming
backend, protocol decoders — "and also try and make it more device agnostic
architecture wise".

## Why not the FPGA

Recorded so it is not revisited. The 5000-sample record is FPGA block RAM and
**there is no external RAM on the board** (teardown, docs/protocol-vds1022.md),
so no bitstream can deepen it. The device is USB Full Speed (~1.2 MB/s
practical, and we already use 683 kB/s), so even perfect firmware caps
continuous streaming near 500 kS/s x 2 channels. Recovering OWON's design from
a Gowin bitstream is a multi-person-year job whose entire payoff is locked to
one model. Protocol fishing is nearly mined out too: the register map is
already recovered from the vendor jar, and the one dormant streaming path in
it (`InfiniteGetData`) was tried by a third party and the device did not
answer.

## What "device agnostic" actually required

The `Backend` trait was already reasonable; `Capabilities` was not. It assumed
a switchable analogue front end (`volts_div`, `probes`) and, load-bearingly,
**a fixed record** — `record_len` propagated into the whole timebase model
(`s/div = record_len / (rate x 10)`), cursors, the shader's x-mapping and CSV
export. The simulator validated none of this because it was built to mirror
the VDS1022.

- `Acquisition::Record { samples }` vs `Acquisition::Stream { chunk }` is now
  a capability. It decides whether spanning more time must cost sample rate.
- `volts_div` may be empty (input range not adjustable).
- `hardware_trigger` says whether the host must find edges itself.
- `record_len()` became an accessor over the acquisition.

`neowon-audio` is the forcing function: a sound card streams, has no trigger
hardware and a fixed range, so porting it is what proves the abstraction is
about instruments rather than about one instrument.

## The timeline

`neowon_dsp::timeline` reduces segments on a real time axis to one min/max
pair per column, with coverage and gap columns. Engine-free, unit-tested, and
the same model serves both acquisition kinds — a streaming source simply never
produces a gap.

Decisions worth keeping:

- **The axis stays proportional.** Butting segments together would look
  tidier, but every Δt, period and frequency reading spanning a join would be
  short by the elapsed dead time.
- **Coverage is a union measure, not a sum**, because estimated timestamps let
  segments overlap; a sum can exceed 1.
- **`-128` is reserved as the gap sentinel** and reduced values clamp to ±127,
  because the averager can genuinely produce -128.
- **Markers are drawn in the compose shader.** `shot`, `NEOWON_SHOT` and the
  MCP screenshot tool read back the display texture only, so gizmo markers
  would be invisible in every capture and unassertable in any test.
- **Min/max pairs index by pair in the shader.** `plot_col(i)` puts every pair
  from the second onward astride a column boundary, smearing gap edges.
- The sentinel cull is `||`, not `&&`: otherwise each gap edge draws a spike
  to the plot top, because `sample_row` clamps rather than culling.

Control model — three bands through `view::hzoom_timeline`, so every entry
point inherits it: timeline on → window widens/narrows, handing back to the
record once it fits; zoom window on → widen toward the record, then engage the
timeline; whole record → engage the timeline. The sample rate is absent from
that path by design; the time base control still walks the rate ladder.

## Decoders

Two layers: `decode::digitize` (hysteresis, because most decode failures are
digitizing failures) and the transport decoders. Every decoder refuses below
`MIN_SAMPLES_BIT = 12` with a message saying what to do, rather than emitting
plausible bytes. UART additionally checks each bit is steady from 20 % to
80 % of its cell — an unstable mid-bit means the baud is wrong.

Bug found by the round trip and worth remembering: samples per bit is rarely
an integer, and resuming the scan from a rounded-down frame end accumulates
lateness until it steps over the next start edge, after which every byte is
wrong *with no error reported*. Resume half a bit inside the stop bit instead.

## Hardware findings this phase (docs/protocol-vds1022.md)

- The **min/max pair phase is not fixed**: consecutive records swap which of
  the pair is the maximum, so consumers must detect it. The naive assumption
  yields a negative peak-to-peak, which is how it was caught.
- **Reads/s is not records/s.** With roll off at 250 kS/s the device yields
  ~36 records/s however fast it is polled, so the 71 % coverage there is a
  device limit. The adaptive backoff still took 200 us/div from ~16 to 100 fps
  and 20 us/div to 125 fps.

## Still open

- Audio backend unverified end to end on macOS: querying an input device
  blocks until microphone access is decided and a CLI binary cannot raise the
  prompt.
- Settings surface and app menu bar (the 2 GB scrollback budget the user
  asked for) — not started.
- Roll still paints whole records instead of scrolling right-to-left (G1).
- Decoder polish: no post-decoder layer (mid-capture baud changes), no
  searchable table, annotations are gizmos so they do not appear in `shot`.
