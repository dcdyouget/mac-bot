//! Native macOS status item used to keep Mac Bot available without a visible window.

use anyhow::{Context as _, Result, anyhow};
use objc2::rc::Retained;
use objc2::{MainThreadMarker, runtime::AnyObject, sel};
use objc2_app_kit::{
    NSApplication, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem, NSVariableStatusItemLength,
};
use objc2_foundation::NSString;

pub struct Tray {
    // NSStatusBar retains the item while installed; these fields also make the
    // lifetime relationship explicit and keep the menu alive for the app.
    _item: Retained<NSStatusItem>,
    _menu: Retained<NSMenu>,
}

impl gpui_kit::Global for Tray {}

impl Tray {
    pub fn install() -> Result<Self> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| anyhow!("menu bar item must be installed on the main thread"))?;
        let application = NSApplication::sharedApplication(mtm);
        let status_bar = NSStatusBar::systemStatusBar();
        let item = status_bar.statusItemWithLength(NSVariableStatusItemLength);
        let button = item
            .button(mtm)
            .context("macOS did not create a status item button")?;
        let app_name = crate::i18n::tr("app.name");
        button.setTitle(&NSString::from_str(app_name.as_ref()));

        let menu = NSMenu::new(mtm);
        let open_item = NSMenuItem::new(mtm);
        let open_title = crate::i18n::tr("action.open");
        unsafe {
            open_item.setTitle(&NSString::from_str(open_title.as_ref()));
            open_item.setTarget(Some(application.as_ref() as &AnyObject));
            open_item.setAction(Some(sel!(unhide:)));
        }
        menu.addItem(&open_item);
        menu.addItem(&NSMenuItem::separatorItem(mtm));

        let quit_item = NSMenuItem::new(mtm);
        let quit_title = crate::i18n::tr("action.quit");
        unsafe {
            quit_item.setTitle(&NSString::from_str(quit_title.as_ref()));
            quit_item.setTarget(Some(application.as_ref() as &AnyObject));
            quit_item.setAction(Some(sel!(terminate:)));
        }
        menu.addItem(&quit_item);
        item.setMenu(Some(&menu));

        Ok(Self {
            _item: item,
            _menu: menu,
        })
    }
}
