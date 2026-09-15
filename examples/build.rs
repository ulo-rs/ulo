fn main() -> Result<(), Box<dyn std::error::Error>> {
    ulo_build::compile_protos("proto/orders.proto")?;
    Ok(())
}
