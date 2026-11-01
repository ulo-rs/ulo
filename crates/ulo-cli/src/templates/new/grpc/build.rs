fn main() -> Result<(), Box<dyn std::error::Error>> {
    // No client: the project serves the service and calls nothing.
    ulo_build::configure().build_client(false).compile(&["proto/greeter.proto"], &["proto"])?;
    println!("cargo::rerun-if-changed=proto/greeter.proto");
    Ok(())
}
