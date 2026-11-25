// The app's icon into the exe (zunflix.rc).
fn main() {
    println!("cargo:rerun-if-changed=zunflix.rc");
    println!("cargo:rerun-if-changed=assets/icon.ico");
    embed_resource::compile("zunflix.rc", embed_resource::NONE).manifest_optional().unwrap();
}
