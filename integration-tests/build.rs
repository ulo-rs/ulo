fn main() -> Result<(), Box<dyn std::error::Error>> {
    // The descriptor set is what a reflection service serves: the compiled
    // schema, so a client can discover the API without holding the `.proto`.
    let descriptor =
        std::path::PathBuf::from(std::env::var("OUT_DIR")?).join("orders_descriptor.bin");
    ulo_build::configure()
        .tonic(|b| b.file_descriptor_set_path(&descriptor))
        .compile_protos(&["proto/orders.proto"], &["proto"])?;

    // A service whose Rust method name and route name diverge, which the proto
    // path cannot produce: prost derives one from the other. `grpc_manual_trait_form`
    // serves it, and it is the shape `#[grpc_stream(...)]` exists for.
    let watcher = tonic_build::manual::Service::builder()
        .name("Watcher")
        .package("ulo_test.watch")
        .method(
            tonic_build::manual::Method::builder()
                .name("watch")
                .route_name("StreamProgress")
                .input_type("crate::grpc_manual_trait_form::msgs::WatchRequest")
                .output_type("crate::grpc_manual_trait_form::msgs::ProgressEvent")
                .codec_path("tonic_prost::ProstCodec")
                .server_streaming()
                .build(),
        )
        .build();
    tonic_build::manual::Builder::new().compile(&[watcher]);
    ulo_build::shapes("ulo_test.watch.Watcher")?;

    Ok(())
}
