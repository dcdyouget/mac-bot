extern crate gpui_kit as gpui;
mod app;
mod i18n;
mod tokens;
mod host_storage;
mod update;
mod computer;

use gpui_kit::*;
use gpui_kit::component::Theme;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(|cx| {
        gpui_kit::init(cx);
        cx.bind_keys([
            KeyBinding::new("cmd-q", app::Quit, None),
            KeyBinding::new("cmd-n", app::New, None),
            KeyBinding::new("cmd-k", app::Search, None),
            KeyBinding::new("cmd-,", app::Settings, None),
            KeyBinding::new("cmd-0", app::MainBot, None),
            KeyBinding::new("cmd-shift-w", app::Workbench, None),
            KeyBinding::new("cmd-shift-u", app::Dashboard, None),
            KeyBinding::new("cmd-shift-s", app::Skills, None),
            KeyBinding::new("cmd-\\", app::ToggleSidebar, None),
            KeyBinding::new("cmd-shift-\\", app::ToggleContext, None),
            KeyBinding::new("escape", app::Back, None),
        ]);
        cx.on_action(|_: &app::Quit, cx| cx.quit());
        let theme = Theme::global_mut(cx);
        theme.font_family = "PingFang SC".into();
        theme.font_size = px(14.);
        tokens::sync_theme(cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        gpui_kit::open_window(WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(800.), px(560.))),
            titlebar: Some(TitlebarOptions { title: Some("Mac Bot".into()), ..Default::default() }),
            ..Default::default()
        }, cx, |window, cx| cx.new(|cx| app::MacBot::new(window,cx)))
        .expect("Unable to open Mac Bot window");
        cx.activate(true);
    });
}
