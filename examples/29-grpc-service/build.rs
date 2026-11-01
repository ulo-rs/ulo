fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/catalog.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/catalog.proto");
    Ok(())
}
