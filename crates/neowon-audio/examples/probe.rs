//! Where does audio input get stuck? Prints each step so a permission
//! prompt or a missing device is obvious.
use cpal::traits::{DeviceTrait, HostTrait};

fn main() {
    println!("1: default_host");
    let host = cpal::default_host();
    println!("2: default_input_device");
    let Some(dev) = host.default_input_device() else {
        println!("   none");
        return;
    };
    println!("3: name = {:?}", dev.name());
    println!("4: default_input_config");
    match dev.default_input_config() {
        Ok(c) => println!(
            "   {:?} {} ch @ {} Hz",
            c.sample_format(),
            c.channels(),
            c.sample_rate().0
        ),
        Err(e) => println!("   err: {e}"),
    }
    println!("5: open backend");
    match neowon_audio::AudioBackend::open() {
        Ok(_) => println!("   OK"),
        Err(e) => println!("   FAILED: {e}"),
    }
}
