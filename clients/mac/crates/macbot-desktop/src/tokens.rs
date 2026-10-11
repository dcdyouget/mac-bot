//! macOS projection of `design/tokens.json`.
//!
//! Keep these values in lockstep with the design source.  `Tokens` carries all
//! palette and scale values so feature views can use a semantic role without
//! inventing platform-specific approximations.

use gpui_kit::App;
use gpui_kit::component::{ActiveTheme, Theme};
use gpui_kit::{Hsla, rgb, rgba};

#[derive(Clone, Copy)]
pub struct Tokens {
    // Surface and interaction colors.
    pub desktop: Hsla,
    pub window: Hsla,
    pub sidebar: Hsla,
    pub panel: Hsla,
    pub chat: Hsla,
    pub elevated: Hsla,
    pub input: Hsla,
    pub hover: Hsla,
    pub active: Hsla,
    pub selected: Hsla,
    pub overlay: Hsla,
    pub border_subtle: Hsla,
    pub border: Hsla,
    pub border_strong: Hsla,
    // Text and action colors.
    pub primary: Hsla,
    pub secondary: Hsla,
    pub tertiary: Hsla,
    pub inverse: Hsla,
    pub link: Hsla,
    pub accent: Hsla,
    pub accent_hover: Hsla,
    pub accent_pressed: Hsla,
    pub accent_soft: Hsla,
    pub accent_on: Hsla,
    // Message surfaces.
    pub bubble_in: Hsla,
    pub bubble_in_text: Hsla,
    pub bubble_out: Hsla,
    pub bubble_out_text: Hsla,
    pub bubble_meta: Hsla,
    pub bubble_out_meta: Hsla,
    // Status colors and their soft backgrounds.
    pub success: Hsla,
    pub success_soft: Hsla,
    pub warning: Hsla,
    pub warning_soft: Hsla,
    pub danger: Hsla,
    pub danger_soft: Hsla,
    pub review: Hsla,
    pub review_soft: Hsla,
    pub code_text: Hsla,
    pub code_bg: Hsla,
    pub chart: [Hsla; 6],
    pub heat: [Hsla; 5],
    pub shadow: Hsla,
    pub avatar: [Hsla; 10],
    pub avatar_eye: Hsla,
    pub avatar_glasses: Hsla,

    // Desktop type scale [size, line height, weight, letter spacing].
    pub desktop_type: [[f32; 4]; 10],
    // Shared layout scales from tokens.json.
    pub space: [f32; 13],
    pub radius: [f32; 12],
    pub motion_ms: [u64; 4],
    pub motion_ease: [&'static str; 3],
    pub motion_spring_sheet: &'static str,
    pub motion_spin_working: &'static str,
    pub blur: [f32; 4],
    pub saturate: f32,
    pub elevation_offset_y: [f32; 5],
    pub elevation_blur: [f32; 5],
    pub elevation_outline: [f32; 5],
    /// Fixed alpha in `elevation.1`; levels 2–4 use the theme shadow token.
    pub elevation_1_shadow_alpha: f32,
    pub font_ui: &'static str,
    /// GPUI's native spelling for the design's leading `-apple-system` font.
    pub font_ui_gpui: &'static str,
    pub font_display: &'static str,
    pub font_mono: &'static str,
    /// The first concrete family in the design's monospace stack.
    pub font_mono_gpui: &'static str,
    pub font_weights: [u16; 4],
    /// Direct projection of all `layout.desktop` keys from `design/tokens.json`.
    pub desktop_layout: DesktopLayout,

    // Compatibility names retained while feature views migrate to semantics.
    pub bot: Hsla,
    pub user: Hsla,
    pub user_text: Hsla,
    pub heatmap_activity: [Hsla; 4],
    pub attention: Hsla,
    pub code: Hsla,
}

#[derive(Clone, Copy)]
pub struct DesktopLayout {
    pub window_min: [f32; 2],
    pub window_default: [f32; 2],
    pub titlebar_height: f32,
    pub traffic_lights_offset: [f32; 2],
    pub sidebar_width: f32,
    pub sidebar_min: f32,
    pub sidebar_max: f32,
    pub listcol_width: f32,
    pub chat_min: f32,
    pub chat_max_content: f32,
    pub panel_width: f32,
    pub panel_min: f32,
    pub panel_max: f32,
    pub divider: f32,
    pub row_chat: f32,
    pub row_nav: f32,
    pub row_compact: f32,
    pub avatar_list: f32,
    pub avatar_message: f32,
    pub avatar_header: f32,
    pub avatar_panel: f32,
    pub composer_min: f32,
    pub composer_max: f32,
}

impl Tokens {
    pub fn get(cx: &App) -> Self {
        let dark = cx.theme().mode.is_dark();
        let translucent = crate::local_settings::translucent()
            && !crate::local_settings::reduce_transparency_enabled();
        let color = |light, dark_color| solid(if dark { dark_color } else { light });
        let alpha = |light, dark_color| rgba(if dark { dark_color } else { light }).into();
        let translucent_surface = |light, dark_color, light_alpha, dark_alpha| {
            if translucent {
                alpha(light_alpha, dark_alpha)
            } else {
                color(light, dark_color)
            }
        };
        let accent = color(0x4a6a9b, 0x8aa6d6);
        let success = color(0x3e8e66, 0x6dbe94);
        let warning = color(0xb7812c, 0xddaa5c);
        let danger = color(0xb9533f, 0xe08470);
        let review = color(0x7b65a8, 0xae9bd6);
        let chart = if dark {
            [0x8aa6d6, 0x7fc2ac, 0xd9b08a, 0xb9a0d4, 0xd98f8f, 0x8e8e93]
        } else {
            [0x4a6a9b, 0x5e9c88, 0xc0956a, 0x9a7bb5, 0xb86b6b, 0x8c8c8c]
        }
        .map(solid);
        let heat = if dark {
            [0x1f1f22, 0x26324a, 0x3a5070, 0x5d7ba8, 0x8aa6d6]
        } else {
            [0xecece8, 0xcdd7e5, 0x9db0cc, 0x6b87b0, 0x4a6a9b]
        }
        .map(solid);
        let avatar = [
            0x7c93b8, 0x86a894, 0xc6a47e, 0xa08dbf, 0xc48a8a, 0x7fa9ae, 0xb5a36c, 0x9b9b9b,
            0xc49ab0, 0x8c9bc4,
        ]
        .map(solid);
        let heatmap_activity = heat[1..5].try_into().unwrap_or([heat[1]; 4]);
        let mut desktop_type = [
            [11., 14., 400., 0.1],
            [12., 16., 400., 0.],
            [13., 18., 400., 0.],
            [14., 20., 400., 0.],
            [13., 18., 500., 0.],
            [15., 20., 600., -0.1],
            [17., 22., 600., -0.2],
            [22., 28., 700., -0.3],
            [28., 34., 700., -0.4],
            [12., 18., 400., 0.],
        ];
        let message_size = crate::local_settings::message_font_size().pixels();
        desktop_type[3][0] = message_size;
        desktop_type[3][1] = 20. * (message_size / 14.);
        Self {
            desktop: color(0xe9e8e4, 0x0b0b0c),
            window: translucent_surface(0xfbfbfa, 0x18181a, 0xfbfbfab8, 0x18181ab3),
            sidebar: translucent_surface(0xf3f3f1, 0x141416, 0xf3f3f194, 0x1414168c),
            panel: translucent_surface(0xf7f7f5, 0x1a1a1d, 0xf7f7f5a3, 0x1a1a1d9e),
            chat: translucent_surface(0xefefeb, 0x111113, 0xefefebdb, 0x111113db),
            elevated: translucent_surface(0xffffff, 0x232326, 0xffffffc7, 0x232326c7),
            input: translucent_surface(0xffffff, 0x1f1f22, 0xffffffcc, 0x1f1f22cc),
            hover: alpha(0x1414180b, 0xffffff0d),
            active: alpha(0x14141813, 0xffffff14),
            selected: alpha(0x4a6a9b1f, 0x8aa6d629),
            overlay: alpha(0x14141847, 0x00000080),
            border_subtle: alpha(0x1414180f, 0xffffff0f),
            border: alpha(0x1414181a, 0xffffff1a),
            border_strong: alpha(0x1414182e, 0xffffff2e),
            primary: color(0x1c1c1e, 0xececee),
            secondary: color(0x6e6e73, 0x9a9aa0),
            tertiary: color(0xa1a1a6, 0x66666b),
            inverse: color(0xffffff, 0x111113),
            link: color(0x3f5f92, 0x9db5de),
            accent,
            accent_hover: color(0x3f5d8a, 0x9db5de),
            accent_pressed: color(0x355079, 0x7a95c4),
            accent_soft: alpha(0x4a6a9b1f, 0x8aa6d629),
            accent_on: color(0xffffff, 0x111113),
            bubble_in: color(0xffffff, 0x222225),
            bubble_in_text: color(0x1c1c1e, 0xececee),
            bubble_out: color(0xdde5f0, 0x2b3a52),
            bubble_out_text: color(0x1c1c1e, 0xeceff5),
            bubble_meta: color(0x8e8e93, 0x7c7c82),
            bubble_out_meta: color(0x6f819c, 0x8fa2c0),
            success,
            success_soft: alpha(0x3e8e661f, 0x6dbe9424),
            warning,
            warning_soft: alpha(0xb7812c21, 0xddaa5c24),
            danger,
            danger_soft: alpha(0xb9533f1c, 0xe0847024),
            review,
            review_soft: alpha(0x7b65a81f, 0xae9bd626),
            code_text: color(0x9a4a5e, 0xe3a3b3),
            code_bg: alpha(0x1414180d, 0xffffff0f),
            chart,
            heat,
            shadow: alpha(0x1414181a, 0x00000073),
            avatar,
            avatar_eye: solid(0x1c1c1e),
            avatar_glasses: solid(0x1c1c1e),
            desktop_type,
            space: [0., 2., 4., 6., 8., 12., 16., 20., 24., 32., 40., 48., 64.],
            radius: [4., 6., 8., 12., 16., 20., 16., 6., 12., 20., 12., 999.],
            motion_ms: [80, 120, 200, 320],
            motion_ease: [
                "cubic-bezier(0.2, 0, 0, 1)",
                "cubic-bezier(0, 0, 0, 1)",
                "cubic-bezier(0.3, 0, 1, 1)",
            ],
            motion_spring_sheet: "stiffness 380, damping 34",
            motion_spin_working: "1200ms linear infinite",
            blur: [40., 40., 24., 30.],
            saturate: 1.8,
            elevation_offset_y: [0., 1., 2., 8., 20.],
            elevation_blur: [0., 2., 8., 24., 60.],
            elevation_outline: [0., 0., 0.5, 0.5, 0.5],
            elevation_1_shadow_alpha: 0.06,
            font_ui: "-apple-system, 'SF Pro Text', 'PingFang SC', 'Helvetica Neue', system-ui, sans-serif",
            font_ui_gpui: ".SystemUIFont",
            font_display: "-apple-system, 'SF Pro Display', 'PingFang SC', system-ui, sans-serif",
            font_mono: "'SF Mono', 'JetBrains Mono', Menlo, monospace",
            font_mono_gpui: "SF Mono",
            font_weights: [400, 500, 600, 700],
            desktop_layout: DesktopLayout {
                window_min: [960., 600.],
                window_default: [1280., 820.],
                titlebar_height: 52.,
                traffic_lights_offset: [20., 18.],
                sidebar_width: 260.,
                sidebar_min: 220.,
                sidebar_max: 320.,
                listcol_width: 260.,
                chat_min: 480.,
                chat_max_content: 760.,
                panel_width: 280.,
                panel_min: 260.,
                panel_max: 360.,
                divider: 1.,
                row_chat: 60.,
                row_nav: 32.,
                row_compact: 28.,
                avatar_list: 40.,
                avatar_message: 32.,
                avatar_header: 32.,
                avatar_panel: 28.,
                composer_min: 44.,
                composer_max: 200.,
            },
            // Legacy aliases used by existing screen implementations.
            bot: color(0xffffff, 0x222225),
            user: color(0xdde5f0, 0x2b3a52),
            user_text: color(0x1c1c1e, 0xeceff5),
            heatmap_activity,
            attention: warning,
            code: color(0x9a4a5e, 0xe3a3b3),
        }
    }

    pub fn bean(&self, index: u64) -> Hsla {
        self.avatar[index as usize % self.avatar.len()]
    }
}

fn solid(hex: u32) -> Hsla {
    rgb(hex).into()
}

pub fn sync_theme(cx: &mut App) {
    let tokens = Tokens::get(cx);
    Theme::update(cx, |theme| {
        theme.background = tokens.window;
        theme.foreground = tokens.primary;
        theme.sidebar = tokens.sidebar;
        theme.muted = tokens.bot;
        theme.muted_foreground = tokens.secondary;

        // Primary controls have separate normal, hover, pressed, and ink
        // colors in GPUI Kit. Keep both semantic primary colors and the
        // button-specific component colors aligned with the design roles.
        theme.primary = tokens.accent;
        theme.primary_hover = tokens.accent_hover;
        theme.primary_active = tokens.accent_pressed;
        theme.primary_foreground = tokens.accent_on;
        theme.button_primary = tokens.accent;
        theme.button_primary_hover = tokens.accent_hover;
        theme.button_primary_active = tokens.accent_pressed;
        theme.button_primary_foreground = tokens.accent_on;

        theme.font_size = gpui_kit::gpui::px(tokens.desktop_type[3][0]);
    });
}
