extern crate gpui_kit as gpui;
mod app;
mod artifact_preview;
mod computer;
mod feature_i18n;
mod features;
mod host_storage;
mod i18n;
mod local_settings;
mod outbox;
mod screen_notice;
mod search_navigation;
mod settings_i18n;
mod settings_view;
mod state_cache;
mod storage_paths;
mod tokens;
mod trace_i18n;
mod trace_view;
#[cfg(target_os = "macos")]
mod tray;
mod update;

use gpui_kit::component::Theme;
use gpui_kit::*;
use std::{cell::Cell, rc::Rc};

struct StyledRoot {
    child: Entity<app::MacBot>,
    message_type: [f32; 4],
}

impl Render for StyledRoot {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .text_size(px(self.message_type[0]))
            .line_height(px(self.message_type[1]))
            .font_weight(gpui_kit::gpui::FontWeight(self.message_type[2]))
            .child(self.child.clone())
    }
}

fn main() {
    let menu_bar_available = Rc::new(Cell::new(false));
    let reopen_menu_bar_available = menu_bar_available.clone();
    let application = gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .with_quit_mode(QuitMode::Explicit);
    application.on_reopen(move |cx| {
        if cx.windows().is_empty() {
            open_main_window(cx, reopen_menu_bar_available.get());
        } else {
            for handle in cx.windows() {
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
            cx.activate(true);
        }
    });
    application.run(move |cx| {
        gpui_kit::init(cx);
        #[cfg(target_os = "macos")]
        {
            match tray::Tray::install() {
                Ok(tray) => {
                    cx.set_global(tray);
                    menu_bar_available.set(true);
                }
                Err(error) => {
                    eprintln!("Unable to install Mac Bot menu bar item: {error:#}");
                }
            }
        }
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
            KeyBinding::new("cmd-e", app::EditAnnouncement, None),
            KeyBinding::new("cmd-\\", app::ToggleSidebar, None),
            KeyBinding::new("cmd-shift-\\", app::ToggleContext, None),
            KeyBinding::new("cmd-i", app::ToggleContext, None),
            KeyBinding::new("cmd-j", app::FocusPending, None),
            KeyBinding::new("f6", app::CycleFocus, None),
            KeyBinding::new("shift-f6", app::CycleFocusBack, None),
            KeyBinding::new("cmd-enter", app::SendMessage, Some("ChatComposer")),
            KeyBinding::new("up", app::FocusPrevMessage, Some("ChatMessage")),
            KeyBinding::new("down", app::FocusNextMessage, Some("ChatMessage")),
            KeyBinding::new("r", app::FocusedMessageReply, Some("ChatMessage")),
            KeyBinding::new("q", app::FocusedMessageReference, Some("ChatMessage")),
            KeyBinding::new("1", app::QuestionOption1, Some("QuestionCard")),
            KeyBinding::new("2", app::QuestionOption2, Some("QuestionCard")),
            KeyBinding::new("3", app::QuestionOption3, Some("QuestionCard")),
            KeyBinding::new("4", app::QuestionOption4, Some("QuestionCard")),
            KeyBinding::new("5", app::QuestionOption5, Some("QuestionCard")),
            KeyBinding::new("6", app::QuestionOption6, Some("QuestionCard")),
            KeyBinding::new("7", app::QuestionOption7, Some("QuestionCard")),
            KeyBinding::new("8", app::QuestionOption8, Some("QuestionCard")),
            KeyBinding::new("9", app::QuestionOption9, Some("QuestionCard")),
            KeyBinding::new("left", app::QuestionPrev, Some("QuestionCard")),
            KeyBinding::new("right", app::QuestionNext, Some("QuestionCard")),
            KeyBinding::new("o", app::QuestionOther, Some("QuestionCard")),
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
        let tokens = tokens::Tokens::get(cx);
        let theme = Theme::global_mut(cx);
        theme.font_family = tokens.font_ui_gpui.into();
        theme.mono_font_family = tokens.font_mono_gpui.into();
        theme.font_size = px(tokens.desktop_type[3][0]);
        theme.mono_font_size = px(tokens.desktop_type[9][0]);
        tokens::sync_theme(cx);
        open_main_window(cx, menu_bar_available.get());
    });
}

fn open_main_window(cx: &mut App, menu_bar_available: bool) {
    let tokens = tokens::Tokens::get(cx);
    let layout = tokens.desktop_layout;
    let message_type = tokens.desktop_type[3];
    let bounds = Bounds::centered(
        None,
        size(px(layout.window_default[0]), px(layout.window_default[1])),
        cx,
    );
    let (handle, _) = gpui_kit::open_window(
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(layout.window_min[0]), px(layout.window_min[1]))),
            titlebar: Some(TitlebarOptions {
                title: Some("Mac Bot".into()),
                ..Default::default()
            }),
            ..Default::default()
        },
        cx,
        move |window, cx| {
            window.on_window_should_close(cx, move |_, cx| {
                let keep_in_menubar = crate::local_settings::keep_in_menubar();
                if keep_in_menubar && menu_bar_available {
                    cx.hide();
                    false
                } else {
                    true
                }
            });
            let child = cx.new(|cx| app::MacBot::new(window, cx));
            cx.new(|_| StyledRoot {
                child,
                message_type,
            })
        },
    )
    .expect("Unable to open Mac Bot window");
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.activate_window());
        cx.activate(true);
    });
}
