//! yamc: a fast, local-first, keyboard-driven mail client.

mod app;
mod body;
mod composer;
mod format;
mod keys;
mod list;
mod settings;
mod theme;
mod thread;

use gpui_kit::*;
use yamc_core::core::{Core, default_data_dir};

gpui_kit::assets::icon_assets!(
    Icons,
    [
        Inbox, FilePen, Settings, ChevronRight, ChevronDown, Search, SearchX, Reply, ReplyAll, Paperclip, Image, Copy, Check,
        Plus, Trash, ArrowLeft, Info, X
    ]
);

fn main() {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .thread_name("yamc-core")
        .build()
        .expect("tokio runtime");
    let data_dir = default_data_dir();
    let core = match Core::open(&data_dir, rt.handle().clone()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("yamc: can't open {}: {e:#}", data_dir.display());
            std::process::exit(1);
        }
    };
    core.start();

    let ui_core = core.clone();
    gpui_kit::application().with_assets(Icons).run(move |cx| {
        gpui_kit::init(cx);
        let s = ui_core.get_settings();
        theme::apply(theme::palette(&s.theme, &s.accent, theme::system_dark(None, cx)), None, cx);
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1240.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("yamc".into()), ..Default::default() }),
            app_id: Some("yamc".into()),
            window_min_size: Some(size(px(640.), px(420.))),
            ..Default::default()
        };
        let core = ui_core.clone();
        gpui_kit::open_window(options, cx, move |window, cx| cx.new(|cx| app::Yamc::new(core, window, cx))).expect("open window");
        cx.activate(true);
    });
    core.stop();
    drop(rt);
}
