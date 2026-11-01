fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/notes.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/notes.proto");
    Ok(())
}
