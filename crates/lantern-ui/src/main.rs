mod app;
mod browse;
mod chat;
mod jobs;
mod pages;
mod theme;
mod view;

fn main() {
    if std::env::args().nth(1).is_some_and(|arg| matches!(arg.as_str(), "--version" | "-V")) {
        println!("Lantern {}", lantern_core::VERSION);
        return;
    }
    theme::brand::register();
    let app = relm4::RelmApp::new(if std::env::var_os("LANTERN_SNAPSHOT").is_some() {
        "dev.megalithofficial.Lantern.Snapshot"
    } else {
        "dev.megalithofficial.Lantern"
    });
    relm4::set_global_css(include_str!("theme/style.css"));
    // The single index from before there were versions moves into the folder of its version.
    if std::env::var_os("LANTERN_DB").is_none() {
        let data = lantern_core::default_data_dir();
        match lantern_core::versions::adopt_legacy(&data) {
            Ok(Some(version)) => {
                let mut settings = lantern_core::settings::Settings::load(&data);
                if settings.version.is_empty() {
                    settings.version = version;
                    let _ = settings.save(&data);
                }
            }
            Ok(None) => {}
            Err(err) => eprintln!("could not move lantern.db into its version folder: {err:#}"),
        }
    }
    app.run::<app::App>(lantern_core::db_path(None));
}
