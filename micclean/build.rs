fn main() {
    embed_resource::compile("assets/ama.rc", embed_resource::NONE).manifest_optional().unwrap();
}
