//! Stateless artifact preview surface for the desktop client.
//!
//! The caller owns fetching, tab selection, retries, and clipboard effects.
//! This component only presents the current artifact and emits copy requests.

use std::rc::Rc;

use gpui_kit::component::{
    Sizable,
    button::{Button, ButtonVariants},
    scroll::ScrollableElement,
    spinner::Spinner,
    text::TextView,
};
use gpui_kit::gpui::{
    App, BoxShadow, FontWeight, IntoElement, ParentElement, RenderOnce, SharedString, Styled,
    Window, div, hsla, point, px,
};
use gpui_kit::prelude::FluentBuilder;

use crate::tokens::Tokens;

type CopyHandler = Rc<dyn Fn(SharedString, &mut Window, &mut App) + 'static>;

/// The kind of text shown by [`ArtifactPreview`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactPreviewFormat {
    /// Render Markdown with the shared text renderer.
    #[default]
    Markdown,
    /// Render source as literal monospaced text.
    PlainText,
}

/// Loading and failure state for an artifact preview.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum ArtifactPreviewState {
    /// The caller is fetching or preparing the artifact content.
    Loading,
    /// The text content is ready to preview.
    #[default]
    Ready,
    /// The preview could not be loaded. The supplied detail should already be localized.
    Error(SharedString),
    /// The file type cannot be previewed. The supplied detail should already be localized.
    Unsupported(SharedString),
}

/// A presentational artifact preview. Network access and clipboard mutations stay
/// with the host view; callbacks are only emitted after the user requests a copy.
#[derive(IntoElement)]
pub struct ArtifactPreview {
    title: SharedString,
    path: SharedString,
    author: SharedString,
    updated_at: SharedString,
    version: Option<u64>,
    content: SharedString,
    format: ArtifactPreviewFormat,
    state: ArtifactPreviewState,
    on_copy_text: Option<CopyHandler>,
    on_copy_path: Option<CopyHandler>,
}

impl ArtifactPreview {
    /// Create a preview for an artifact title and its project-relative path.
    pub fn new(title: impl Into<SharedString>, path: impl Into<SharedString>) -> Self {
        Self {
            title: title.into(),
            path: path.into(),
            author: SharedString::default(),
            updated_at: SharedString::default(),
            version: None,
            content: SharedString::default(),
            format: ArtifactPreviewFormat::default(),
            state: ArtifactPreviewState::default(),
            on_copy_text: None,
            on_copy_path: None,
        }
    }

    /// Set the human-readable author name shown next to the artifact metadata.
    pub fn author(mut self, author: impl Into<SharedString>) -> Self {
        self.author = author.into();
        self
    }

    /// Set the already formatted update time, such as `10:18`.
    pub fn updated_at(mut self, updated_at: impl Into<SharedString>) -> Self {
        self.updated_at = updated_at.into();
        self
    }

    /// Set the artifact version number.
    pub fn version(mut self, version: u64) -> Self {
        self.version = Some(version);
        self
    }

    /// Set text content and whether it should be rendered as Markdown or source.
    pub fn content(
        mut self,
        content: impl Into<SharedString>,
        format: ArtifactPreviewFormat,
    ) -> Self {
        self.content = content.into();
        self.format = format;
        self
    }

    /// Set the visible loading, error, unsupported, or ready state.
    pub fn state(mut self, state: ArtifactPreviewState) -> Self {
        self.state = state;
        self
    }

    /// Receive the text the user asked to copy. The host decides how to write it
    /// to the clipboard and how to report success.
    pub fn on_copy_text(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy_text = Some(Rc::new(handler));
        self
    }

    /// Receive the artifact path the user asked to copy.
    pub fn on_copy_path(
        mut self,
        handler: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_copy_path = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for ArtifactPreview {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let tokens = Tokens::get(cx);
        let title_type = tokens.desktop_type[5];
        let footnote_type = tokens.desktop_type[1];
        let meta = div()
            .flex()
            .flex_col()
            .gap(px(tokens.space[2]))
            .min_w_0()
            .child(
                div()
                    .text_size(px(title_type[0]))
                    .line_height(px(title_type[1]))
                    .font_weight(FontWeight(title_type[2]))
                    .text_color(tokens.primary)
                    .child(self.title.clone()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(px(tokens.space[4]))
                    .text_size(px(footnote_type[0]))
                    .line_height(px(footnote_type[1]))
                    .font_weight(FontWeight(footnote_type[2]))
                    .text_color(tokens.secondary)
                    .when(!self.author.is_empty(), |row| {
                        row.child(format!(
                            "{} {}",
                            self.author,
                            crate::i18n::tr("artifact.provided_by")
                        ))
                    })
                    .when(!self.path.is_empty(), |row| {
                        row.child(
                            div()
                                .font_family(tokens.font_mono_gpui)
                                .text_color(tokens.code_text)
                                .child(self.path.clone()),
                        )
                    })
                    .when(!self.updated_at.is_empty(), |row| {
                        row.child(format!(
                            "{} {}",
                            crate::i18n::tr("artifact.preview.updated"),
                            self.updated_at
                        ))
                    })
                    .when_some(self.version, |row, version| {
                        let label = crate::i18n::tr("artifact.preview.version")
                            .to_string()
                            .replace("{n}", &version.to_string());
                        row.child(label)
                    }),
            );

        let mut actions = div().flex().items_center().gap(px(tokens.space[4]));
        if let Some(handler) = self.on_copy_path.clone().filter(|_| !self.path.is_empty()) {
            let path = self.path.clone();
            actions = actions.child(
                Button::new("artifact-preview-copy-path")
                    .secondary()
                    .small()
                    .label(crate::i18n::tr("artifact.preview.copy_path"))
                    .on_click(move |_, window, cx| handler(path.clone(), window, cx)),
            );
        }
        if let Some(handler) = self.on_copy_text.clone()
            && matches!(&self.state, ArtifactPreviewState::Ready)
        {
            let content = self.content.clone();
            actions = actions.child(
                Button::new("artifact-preview-copy-text")
                    .ghost()
                    .small()
                    .label(crate::i18n::tr("artifact.preview.copy_text"))
                    .on_click(move |_, window, cx| handler(content.clone(), window, cx)),
            );
        }

        let header = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(tokens.space[5]))
            .p(px(tokens.space[6]))
            .border_b(px(tokens.desktop_layout.divider))
            .border_color(tokens.border_subtle)
            .bg(tokens.panel)
            .child(meta)
            .child(actions);

        let content = match self.state {
            ArtifactPreviewState::Loading => div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(tokens.space[5]))
                .flex_1()
                .text_size(px(footnote_type[0]))
                .line_height(px(footnote_type[1]))
                .font_weight(FontWeight(footnote_type[2]))
                .text_color(tokens.secondary)
                .child(Spinner::new().color(tokens.accent))
                .child(crate::i18n::tr("artifact.preview.loading"))
                .into_any_element(),
            ArtifactPreviewState::Error(detail) => status_view(
                crate::i18n::tr("artifact.preview.error"),
                detail,
                tokens.danger,
                tokens.secondary,
                &tokens,
            ),
            ArtifactPreviewState::Unsupported(detail) => status_view(
                crate::i18n::tr("artifact.preview.unsupported"),
                detail,
                tokens.secondary,
                tokens.tertiary,
                &tokens,
            ),
            ArtifactPreviewState::Ready => {
                let preview_id =
                    SharedString::from(format!("artifact-preview-content-{}", self.title));
                let surface = div()
                    .w_full()
                    .max_w(px(tokens.desktop_layout.chat_max_content))
                    .min_h_0()
                    .p(px(tokens.space[10]))
                    .rounded(px(tokens.radius[3]))
                    .border(px(tokens.desktop_layout.divider))
                    .border_color(tokens.border_subtle)
                    .bg(tokens.chat);
                surface.style().box_shadow = Some(vec![BoxShadow {
                    color: hsla(0., 0., 0., tokens.elevation_1_shadow_alpha),
                    offset: point(px(0.), px(tokens.elevation_offset_y[1])),
                    blur_radius: px(tokens.elevation_blur[1]),
                    spread_radius: px(0.),
                }]);
                match self.format {
                    ArtifactPreviewFormat::Markdown => surface
                        .child(TextView::markdown(preview_id, self.content))
                        .into_any_element(),
                    ArtifactPreviewFormat::PlainText => surface
                        .child(
                            div()
                                .font_family(tokens.font_mono_gpui)
                                .text_size(px(tokens.desktop_type[9][0]))
                                .line_height(px(tokens.desktop_type[9][1]))
                                .font_weight(FontWeight(tokens.desktop_type[9][2]))
                                .text_color(tokens.primary)
                                .child(self.content),
                        )
                        .into_any_element(),
                }
            }
        };

        div()
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(tokens.chat)
            .child(header)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .p(px(tokens.space[9]))
                    .flex()
                    .justify_center()
                    .child(content),
            )
    }
}

fn status_view(
    title: SharedString,
    detail: SharedString,
    color: gpui_kit::Hsla,
    detail_color: gpui_kit::Hsla,
    tokens: &Tokens,
) -> gpui_kit::gpui::AnyElement {
    let title_type = tokens.desktop_type[5];
    let footnote_type = tokens.desktop_type[1];
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(tokens.space[4]))
        .flex_1()
        .text_center()
        .child(
            div()
                .text_size(px(title_type[0]))
                .line_height(px(title_type[1]))
                .font_weight(FontWeight(title_type[2]))
                .text_color(color)
                .child(title),
        )
        .child(
            div()
                .text_size(px(footnote_type[0]))
                .line_height(px(footnote_type[1]))
                .font_weight(FontWeight(footnote_type[2]))
                .text_color(detail_color)
                .child(detail),
        )
        .into_any_element()
}
