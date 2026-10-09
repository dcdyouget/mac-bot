use gpui_kit::App;
use gpui_kit::component::{ActiveTheme, Theme};
use gpui_kit::{Hsla, rgb};

#[derive(Clone, Copy)]
pub struct Tokens {
    pub window: Hsla,
    pub sidebar: Hsla,
    pub bot: Hsla,
    pub user: Hsla,
    pub user_text: Hsla,
    pub primary: Hsla,
    pub secondary: Hsla,
    pub accent: Hsla,
    pub success: Hsla,
    pub heatmap_activity: [Hsla; 4],
    pub attention: Hsla,
    pub danger: Hsla,
    pub code: Hsla,
    pub border: Hsla,
}
impl Tokens {
    pub fn get(cx: &App) -> Self {
        let dark = cx.theme().mode.is_dark();
        let color = |light, dark_color| rgb(if dark { dark_color } else { light }).into();
        Self {
            window: color(0xffffff, 0x1c1c1e),
            sidebar: color(0xf5f5f7, 0x232325),
            bot: color(0xefeff1, 0x2c2c2e),
            user: color(0x111111, 0xf2f2f2),
            user_text: color(0xffffff, 0x111111),
            primary: color(0x1d1d1f, 0xf5f5f7),
            secondary: color(0x86868b, 0x98989d),
            accent: color(0x2f7bf6, 0x4c8dff),
            success: rgb(0x34c759).into(),
            heatmap_activity: Self::activity_colors(),
            attention: rgb(0xff9f0a).into(),
            danger: rgb(0xff3b30).into(),
            code: color(0xd63a5b, 0xff6b8a),
            border: cx.theme().border,
        }
    }
    pub fn activity_colors() -> [Hsla; 4] {
        let activity: Hsla = rgb(0x34c759).into();
        [0.35, 0.55, 0.75, 1.0].map(|opacity| activity.opacity(opacity))
    }
    pub fn bean(color: u64) -> Hsla {
        const COLORS: [u32; 10] = [
            0x9d78d5, 0xffa163, 0x648ade, 0x75ba9a, 0xf07999, 0x75bccc, 0xc1b96d, 0xb9907d,
            0x8494ad, 0xb88bb7,
        ];
        rgb(COLORS[color as usize % COLORS.len()]).into()
    }
}
pub fn sync_theme(cx: &mut App) {
    let t = Tokens::get(cx);
    let theme = Theme::global_mut(cx);
    theme.background = t.window;
    theme.foreground = t.primary;
    theme.sidebar = t.sidebar;
    theme.muted = t.bot;
    theme.muted_foreground = t.secondary;
    theme.primary = t.accent;
}
