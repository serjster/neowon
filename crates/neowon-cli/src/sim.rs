//! `neowon sim …`: simulator output for determinism checks. Nothing
//! here touches USB.

use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Subcommand;
use neowon_sim::IqScene;
use neowon_sim::dataset::{self, Recipe};
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
    /// Build a labelled dataset (Phase 10.8) as a SigMF recording in `out`
    Dataset {
        /// Recipe JSON (omitted fields take the defaults)
        #[arg(long)]
        recipe: Option<PathBuf>,
        /// Overrides the recipe's seed
        #[arg(long)]
        seed: Option<u64>,
        /// Overrides the recipe's example count
        #[arg(long)]
        examples: Option<usize>,
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
        SimCmd::Dataset {
            recipe,
            seed,
            examples,
            out,
        } => {
            let mut r = match recipe {
                Some(p) => {
                    let text = std::fs::read_to_string(p)
                        .with_context(|| format!("reading {}", p.display()))?;
                    Recipe::from_json(&text).map_err(anyhow::Error::msg)?
                }
                None => Recipe::default(),
            };
            r.seed = seed.unwrap_or(r.seed);
            r.examples = examples.unwrap_or(r.examples);
            let (data, meta) = dataset::sigmf(&r).map_err(anyhow::Error::msg)?;
            std::fs::create_dir_all(out)?;
            std::fs::write(out.join(format!("{}.sigmf-data", r.name)), &data)?;
            std::fs::write(out.join(format!("{}.sigmf-meta", r.name)), &meta)?;
            println!(
                r#"{{"name":"{}","seed":{},"examples":{},"bytes_fnv":{}}}"#,
                r.name,
                r.seed,
                r.examples,
                fnv1a64(&data)
            );
            Ok(())
        }
    }
}
