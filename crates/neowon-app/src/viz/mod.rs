//! Parallel visualizations driven by the same acquisition frames as the
//! phosphor plot: the realtime waterfall spectrogram and the 3D signal
//! viewport. The 2D plot stays the precision instrument; everything here
//! is an extra view, off by default, fully script-controlled.

pub mod three_d;
pub mod waterfall;
