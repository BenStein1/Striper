//! Feed raw 16 kHz mono f32le audio through the actual installed CPU backend.
//! Usage: cargo run --example probe -- LIBRARY MODEL AUDIO.f32
use std::path::Path;
fn main() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 4 {
        return Err("Expected LIBRARY MODEL AUDIO.f32".into());
    }
    let mut detector = striper_listener::Detector::open(Path::new(&args[1]), Path::new(&args[2]))?;
    let bytes = std::fs::read(&args[3]).map_err(|e| e.to_string())?;
    let mut samples: Vec<f32> = bytes
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    samples.extend(std::iter::repeat_n(0.0, 32_000));
    let mut woke = false;
    for chunk in samples.chunks(512) {
        let d = detector.feed(chunk)?;
        if !woke && striper_listener::wake_prefix(&d.text).is_some() {
            println!("WAKE: {}", d.text);
            woke = true;
        }
        if d.endpoint {
            println!("FINAL: {}", d.text);
        }
    }
    if !woke {
        return Err("No wake phrase detected".into());
    }
    Ok(())
}
