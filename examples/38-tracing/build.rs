fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/pricing.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/pricing.proto");
    Ok(())
}
