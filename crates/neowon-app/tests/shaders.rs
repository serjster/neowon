//! Parse and validate every WGSL shader without a GPU (naga matches the wgpu
//! version Bevy 0.19 ships).

use naga::valid::{Capabilities, ValidationFlags, Validator};

#[test]
fn all_wgsl_shaders_validate() {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src/shaders");
    let mut checked = 0;
    for entry in std::fs::read_dir(dir).expect("shaders dir") {
        let path = entry.unwrap().path();
        if path.extension().is_none_or(|e| e != "wgsl") {
            continue;
        }
        let src = std::fs::read_to_string(&path).unwrap();
        let module = naga::front::wgsl::parse_str(&src)
            .unwrap_or_else(|e| panic!("{}: parse error:\n{}", path.display(), e));
        Validator::new(ValidationFlags::all(), Capabilities::all())
            .validate(&module)
            .unwrap_or_else(|e| panic!("{}: validation error:\n{:?}", path.display(), e));
        checked += 1;
    }
    assert!(checked > 0, "no shaders found in {dir}");
}
