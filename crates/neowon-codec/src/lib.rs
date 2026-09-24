//! Engine-free audio codecs and DAB+ transport framing.
//!
//! This crate owns the part of DAB that is *after* the sub-channel bytes:
//! the DAB+ audio superframe and its Reed–Solomon protection
//! ([`dabplus`]), the in-band PAD it carries, and the audio codec
//! adapters (HE-AAC v2 for DAB+, MPEG-1 Layer II for DAB classic —
//! [`aac`] and [`mp2`]). It carries no Bevy/GPU dependency and no device
//! access; the app wires its output into `neowon-audio`'s sink.
//!
//! The HE-AAC v2 decoder is the pure-Rust
//! MIT `oxideav-aac` by default, with the `fdk-aac` C binding (MIT
//! binding over Fraunhofer's BSD-based libfdk-aac, no patent grant) as
//! the feature-gated fallback that decodes real DAB+ — the 960-line
//! transform with SBR and PS, which `oxideav-aac` 0.1.7 rejects.
//! `AacDecoder::BACKEND` names the compiled-in one. MP2 is
//! `oxideav-mp2`. Both live behind this crate's own types.
//!
//! # Standards map (constants cite these clauses)
//!
//! * **ETSI TS 102 563 V2.1.1** — DAB+ audio: superframe size and AU
//!   mapping (clause 5.1/5.2), per-AU CRC (clause 5.2, procedure per
//!   EN 300 401 annex E), PAD carriage (clause 5.4), RS(120,110) and
//!   the virtual interleaver (clauses 6.1–6.5), signalling of the audio
//!   parameters the AudioSpecificConfig is derived from (clause 7.2).
//! * **ETSI EN 300 401 V2.1.1** — DAB system: CRC-16 procedure
//!   (annex E), PAD structure (clause 7.4), DAB audio frame (MP2).
//! * **ISO/IEC 14496-3** — AAC `AudioSpecificConfig` (clause 1.6.2.1)
//!   and `data_stream_element()` (clause 4.4.2.5). The standard text is
//!   not freely distributable; the bit layouts used here are pinned
//!   against the pinned `oxideav-aac` 0.1.7 implementation (MIT), which
//!   the adapter round-trips through in its tests.
//!
//! See `docs/tasks/phase10-dab-spec.md` §10.15.3 for the contract and the
//! verification rows this crate must satisfy.

#![warn(missing_debug_implementations)]

pub mod aac;
pub mod dabplus;
mod error;
pub mod mp2;

pub use error::Error;

pub type Result<T> = std::result::Result<T, Error>;
