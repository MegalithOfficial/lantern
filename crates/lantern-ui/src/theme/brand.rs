use gtk::{gio, glib, prelude::*};

pub fn register() {
    gio::resources_register_include!("lantern.gresource").expect("registering the embedded Lantern icon");
}

pub fn image(size: i32) -> gtk::Image {
    let svg = include_str!("../../../../assets/lantern-icon.svg").replace("#1f282d", "#f0f6fc");
    let image = gtk::Image::from_paintable(Some(&gtk::Svg::from_bytes(&glib::Bytes::from_owned(svg.into_bytes()))));
    image.set_pixel_size(size);
    image.set_tooltip_text(Some("Lantern"));
    image
}
