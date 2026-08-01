fn main() {
    println!("cargo:rerun-if-changed=ui");
    println!("cargo:rerun-if-changed=static");
    println!("cargo:rerun-if-changed=data/base-filaments-index.csv");
    println!("cargo:rerun-if-changed=data/bambu-color-names.csv");

    slint_build::compile_with_config(
        "ui/appwindow.slint",
        slint_build::CompilerConfiguration::new().embed_resources(slint_build::EmbedResourcesKind::EmbedForSoftwareRenderer),
    )
    .unwrap();
}
