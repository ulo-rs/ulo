fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::configure().compile(&["proto/grpc/testing/test.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto");
    Ok(())
}
