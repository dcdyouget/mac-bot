extern crate gpui_kit as gpui;
mod app;
mod computer;
mod feature_i18n;
mod features;
mod host_storage;
mod i18n;
mod local_settings;
mod outbox;
mod settings_i18n;
mod settings_view;
mod state_cache;
mod storage_paths;
mod tokens;
mod trace_i18n;
mod trace_view;
mod update;

use gpui_kit::component::Theme;
use gpui_kit::*;

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(|cx| {
            gpui_kit::init(cx);
            cx.bind_keys([
                KeyBinding::new("cmd-q", app::Quit, None),
                KeyBinding::new("cmd-n", app::New, None),
                KeyBinding::new("cmd-k", app::Search, None),
                KeyBinding::new("cmd-,", app::Settings, None),
                KeyBinding::new("cmd-0", app::MainBot, None),
                KeyBinding::new("cmd-1", app::Chat1, None),
                KeyBinding::new("cmd-2", app::Chat2, None),
                KeyBinding::new("cmd-3", app::Chat3, None),
                KeyBinding::new("cmd-4", app::Chat4, None),
                KeyBinding::new("cmd-5", app::Chat5, None),
                KeyBinding::new("cmd-6", app::Chat6, None),
                KeyBinding::new("cmd-7", app::Chat7, None),
                KeyBinding::new("cmd-8", app::Chat8, None),
                KeyBinding::new("cmd-9", app::Chat9, None),
                KeyBinding::new("cmd-shift-w", app::Workbench, None),
                KeyBinding::new("cmd-shift-u", app::Dashboard, None),
                KeyBinding::new("cmd-shift-s", app::Skills, None),
                KeyBinding::new("cmd-\\", app::ToggleSidebar, None),
                KeyBinding::new("cmd-shift-\\", app::ToggleContext, None),
                KeyBinding::new("escape", app::Back, None),
            ]);
            cx.on_action(|_: &app::Quit, cx| cx.quit());
            cx.set_menus([
                Menu::new("Mac Bot").items([
                    MenuItem::action(i18n::tr("nav.settings"), app::Settings),
                    MenuItem::separator(),
                    MenuItem::action(i18n::tr("action.quit"), app::Quit),
                ]),
                Menu::new(i18n::tr("nav.navigate")).items([
                    MenuItem::action(i18n::tr("nav.new"), app::New),
                    MenuItem::action(i18n::tr("nav.search"), app::Search),
                    MenuItem::action(i18n::tr("nav.workbench"), app::Workbench),
                    MenuItem::action(i18n::tr("nav.dashboard"), app::Dashboard),
                    MenuItem::action(i18n::tr("nav.skills"), app::Skills),
                ]),
            ]);
            let theme = Theme::global_mut(cx);
            theme.font_family = "PingFang SC".into();
            theme.font_size = px(14.);
            tokens::sync_theme(cx);
            let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
            gpui_kit::open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(800.), px(560.))),
                    titlebar: Some(TitlebarOptions {
                        title: Some("Mac Bot".into()),
                        ..Default::default()
                    }),
                    ..Default::default()
                },
                cx,
                |window, cx| cx.new(|cx| app::MacBot::new(window, cx)),
            )
            .expect("Unable to open Mac Bot window");
            cx.activate(true);
        });
}
