fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/clock.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/clock.proto");
    Ok(())
}
