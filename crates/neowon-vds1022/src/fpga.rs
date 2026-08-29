//! FPGA bitstream selection. The host must upload a bitstream at every
//! device cold start; which one depends on the hardware version string read
//! from flash. Bitstream files are vendor blobs and are NOT redistributed
//! with neowon — point at an OWON-VDS1022 checkout's `fwr/` directory.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Map the hardware version string to the FPGA generation number
/// (`SoftwareControl.setBoardVersion` in the vendor app).
pub fn fpga_generation(hw_version: &str) -> Result<u32> {
    let v = hw_version.trim();
    if v.starts_with("V2.7.0") {
        return Ok(3);
    }
    if v.starts_with("V2.4.623") || v.starts_with("V2.6.0") {
        return Ok(2);
    }
    if v.starts_with("V2.") || v.starts_with("V1.") {
        return Ok(1);
    }
    // "V<n>." — newer boards name their generation directly.
    if let Some(rest) = v.strip_prefix('V')
        && let Some(dot) = rest.find('.')
        && let Ok(n) = rest[..dot].parse::<u32>()
    {
        return Ok(n);
    }
    Err(Error::Fpga(format!("cannot derive FPGA generation from version {v:?}")))
}

/// Find `VDS1022_FPGAV{n}_*.bin` in `dir`.
pub fn find_bitstream(dir: &Path, generation: u32) -> Result<PathBuf> {
    let prefix = format!("VDS1022_FPGAV{generation}").to_uppercase();
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_uppercase())
            .unwrap_or_default();
        if name.starts_with(&prefix) && name.ends_with(".BIN") {
            return Ok(path);
        }
    }
    Err(Error::Fpga(format!(
        "no {prefix}*.bin in {} — pass --fpga-dir pointing at OWON-VDS1022/fwr",
        dir.display()
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generations() {
        assert_eq!(fpga_generation("V2.7.0").unwrap(), 3);
        assert_eq!(fpga_generation("V2.6.0").unwrap(), 2);
        assert_eq!(fpga_generation("V2.4.623").unwrap(), 2);
        assert_eq!(fpga_generation("V2.5").unwrap(), 1);
        assert_eq!(fpga_generation("V1.0").unwrap(), 1);
        assert_eq!(fpga_generation("V4.2").unwrap(), 4);
        assert!(fpga_generation("garbage").is_err());
    }
}
