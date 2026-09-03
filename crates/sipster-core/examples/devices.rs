//! Lists the audio devices Sipster can use for calls.
//!
//! `cargo run -p sipster-core --example devices`

fn main() {
    println!("input (microphone):");
    for (id, name) in sipster_core::audio::input_devices() {
        println!("  {id}  —  {name}");
    }

    println!("\noutput (speaker):");
    for (id, name) in sipster_core::audio::output_devices() {
        println!("  {id}  —  {name}");
    }
}
