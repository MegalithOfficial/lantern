fn main() {
    glib_build_tools::compile_resources(&["../../assets"], "resources.gresource.xml", "lantern.gresource");
}
