//! How long one 64 Ki-pair frame of the `rf-digital` scene takes to
//! generate, against its 32 ms real-time budget at 2.048 MS/s.
fn main() {
    let scene = neowon_sim::RfScene::preset("rf-digital")
        .unwrap()
        .baseband(100e6, 2.048e6, 0.0);
    let t = std::time::Instant::now();
    let n = 64 * 1024;
    let v = scene.samples(1, 0, n);
    let ms = t.elapsed().as_secs_f64() * 1e3;
    println!(
        "{} pairs in {ms:.1} ms (budget {:.1} ms); first {:?}",
        n,
        n as f64 / 2.048e6 * 1e3,
        &v[..2]
    );
}
