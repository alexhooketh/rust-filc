//! Runs a small end-to-end demonstration of the extern-like Fil-C client.

use filc_counter::bridge;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let counter = bridge::counter_new(10)?;
    println!("20 + 22 = {}", bridge::add(20, 22)?);
    println!("reverse = {:?}", bridge::reverse(b"Fil-C")?);
    println!("{}", bridge::greet("Rust")?);
    println!("counter = {}", bridge::counter_add(&counter, 5)?);
    Ok(())
}
