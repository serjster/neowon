//! `neowon sim …`: simulator output for determinism checks (D8). Nothing
//! here touches USB.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use neowon_sim::IqScene;
use neowon_sim::iq::{fnv1a64, to_le_bytes};

#[derive(Subcommand)]
pub enum SimCmd {
    /// Write the reference IQ scene as little-endian f32, interleaved I,Q
    Iq {
        #[arg(long, default_value_t = 1)]
        seed: u64,
        /// Number of I/Q pairs
        #[arg(long, default_value_t = 1024)]
        n: usize,
        #[arg(long)]
        out: PathBuf,
    },
}

pub fn run(cmd: &SimCmd) -> Result<()> {
    match cmd {
        SimCmd::Iq { seed, n, out } => {
            let bytes = to_le_bytes(&IqScene::reference().samples(*seed, 0, *n));
            std::fs::write(out, &bytes).with_context(|| format!("writing {}", out.display()))?;
            // The same readout `get iq` serves.
            println!(
                r#"{{"seed":{seed},"n":{n},"layout":"complex","bytes_fnv":{}}}"#,
                fnv1a64(&bytes)
            );
            Ok(())
        }
    }
}
