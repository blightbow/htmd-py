use pyo3::prelude::*;

use htmd_lib::element_handler::{HandlerResult, Handlers};
use htmd_lib::options::{
    BrStyle as HtmdBrStyle, BulletListMarker as HtmdBulletListMarker,
    CodeBlockFence as HtmdCodeBlockFence, CodeBlockStyle as HtmdCodeBlockStyle,
    HeadingStyle as HtmdHeadingStyle, HrStyle as HtmdHrStyle,
    LinkReferenceStyle as HtmdLinkReferenceStyle, LinkStyle as HtmdLinkStyle,
    Options as HtmdOptions,
};
use htmd_lib::{Element, HtmlToMarkdownBuilder};

/// Python class that mirrors htmd's `Options`
#[pyclass(name = "Options", from_py_object)]
#[derive(Clone)]
pub struct PyOptions {
    #[pyo3(get, set)]
    pub heading_style: String,
    #[pyo3(get, set)]
    pub hr_style: String,
    #[pyo3(get, set)]
    pub br_style: String,
    #[pyo3(get, set)]
    pub link_style: String,
    #[pyo3(get, set)]
    pub link_reference_style: String,
    #[pyo3(get, set)]
    pub code_block_style: String,
    #[pyo3(get, set)]
    pub code_block_fence: String,
    #[pyo3(get, set)]
    pub bullet_list_marker: String,
    #[pyo3(get, set)]
    pub preformatted_code: bool,

    // Special attributes that don't map directly to HtmdOptions
    #[pyo3(get, set)]
    pub skip_tags: Vec<String>,

    // --- Text-only handler knobs -----------------------------------------
    //
    // These configure preset `add_handler` callbacks that htmd-py installs
    // on the builder when apply_to_builder runs. They express the common
    // "render text-only markdown" pattern (image alt text instead of image
    // markdown, drop links that contain only an image) that projects doing
    // LLM data ingestion and RAG generally want.
    //
    // The handler installation happens Rust-side so per-element callbacks
    // never cross the Python/Rust boundary; these options are read once at
    // convert time and compiled into a fast inner loop.
    /// Custom image replacement template. When set, `<img>` elements are
    /// replaced with this string after substituting `{alt}` for the alt
    /// attribute's value. When `None`, htmd's default image rendering
    /// (`![alt](src)`) is used unless `drop_empty_alt_images` is set.
    #[pyo3(get, set)]
    pub image_placeholder: Option<String>,
    /// When true, `<img>` elements whose alt attribute is empty or missing
    /// are dropped from the output entirely. Works both with a custom
    /// `image_placeholder` template and with htmd's default image handler.
    #[pyo3(get, set)]
    pub drop_empty_alt_images: bool,
    /// When true, `<a>` elements whose inner content is a single image
    /// render (either the `image_placeholder` template's literal prefix
    /// or the markdown image marker `![`) are unwrapped: the image render
    /// is emitted without the surrounding link. Useful for LLM pipelines
    /// that want caption text without the surrounding navigation link.
    #[pyo3(get, set)]
    pub drop_image_only_links: bool,
}

impl PyOptions {
    /// Convert PyOptions to HtmdOptions
    pub fn to_htmd_options(&self) -> HtmdOptions {
        let heading_style = match self.heading_style.as_str() {
            "setex" => HtmdHeadingStyle::Setex,
            _ => HtmdHeadingStyle::Atx,
        };

        let hr_style = match self.hr_style.as_str() {
            "dashes" => HtmdHrStyle::Dashes,
            "underscores" => HtmdHrStyle::Underscores,
            _ => HtmdHrStyle::Asterisks,
        };

        let br_style = match self.br_style.as_str() {
            "backslash" => HtmdBrStyle::Backslash,
            _ => HtmdBrStyle::TwoSpaces,
        };

        let link_style = match self.link_style.as_str() {
            "referenced" => HtmdLinkStyle::Referenced,
            _ => HtmdLinkStyle::Inlined,
        };

        let link_reference_style = match self.link_reference_style.as_str() {
            "collapsed" => HtmdLinkReferenceStyle::Collapsed,
            "shortcut" => HtmdLinkReferenceStyle::Shortcut,
            _ => HtmdLinkReferenceStyle::Full,
        };

        let code_block_style = match self.code_block_style.as_str() {
            "indented" => HtmdCodeBlockStyle::Indented,
            _ => HtmdCodeBlockStyle::Fenced,
        };

        let code_block_fence = match self.code_block_fence.as_str() {
            "tildes" => HtmdCodeBlockFence::Tildes,
            _ => HtmdCodeBlockFence::Backticks,
        };

        let bullet_list_marker = match self.bullet_list_marker.as_str() {
            "dash" => HtmdBulletListMarker::Dash,
            _ => HtmdBulletListMarker::Asterisk,
        };

        // htmd 0.5 added three fields to `Options` that 0.1 didn't have:
        // `ul_bullet_spacing`, `ol_number_spacing`, and `translation_mode`.
        // Seed them from `Options::default()` so the PyOptions surface stays
        // source-compatible with 0.1's wrapper. A future PR can expose these
        // to Python if there's a user request.
        let defaults = HtmdOptions::default();
        HtmdOptions {
            heading_style,
            hr_style,
            br_style,
            link_style,
            link_reference_style,
            code_block_style,
            code_block_fence,
            bullet_list_marker,
            ul_bullet_spacing: defaults.ul_bullet_spacing,
            ol_number_spacing: defaults.ol_number_spacing,
            preformatted_code: self.preformatted_code,
            translation_mode: defaults.translation_mode,
        }
    }

    /// Apply the options to an HtmlToMarkdownBuilder
    pub fn apply_to_builder(&self, builder: HtmlToMarkdownBuilder) -> HtmlToMarkdownBuilder {
        let mut builder = builder.options(self.to_htmd_options());

        // Apply skip_tags if any
        if !self.skip_tags.is_empty() {
            let skip_tags: Vec<&str> = self.skip_tags.iter().map(|s| s.as_str()).collect();
            builder = builder.skip_tags(skip_tags);
        }

        // Install a custom `<img>` handler when any image-related knob is
        // active. Returning `None` from a handler tells htmd to emit nothing
        // for that element; returning `Some(HandlerResult::from(s))` inserts
        // `s` as already-translated markdown (no bracket escaping, unlike
        // text node serialization).
        let wants_img_handler =
            self.image_placeholder.is_some() || self.drop_empty_alt_images;
        if wants_img_handler {
            let template = self.image_placeholder.clone();
            let drop_empty = self.drop_empty_alt_images;
            builder = builder.add_handler(
                vec!["img"],
                move |handlers: &dyn Handlers, element: Element| -> Option<HandlerResult> {
                    let alt = element
                        .attrs
                        .iter()
                        .find(|a| &*a.name.local == "alt")
                        .map(|a| a.value.to_string())
                        .unwrap_or_default();
                    let alt_trimmed = alt.trim();

                    if alt_trimmed.is_empty() && drop_empty {
                        return Some(HandlerResult::from(String::new()));
                    }

                    if let Some(ref t) = template {
                        let replaced = t.replace("{alt}", alt_trimmed);
                        return Some(HandlerResult::from(replaced));
                    }

                    // `drop_empty_alt_images` active but no custom template:
                    // defer to htmd's built-in img handler for non-empty alts
                    // so we get the standard `![alt](src)` rendering.
                    handlers.fallback(element)
                },
            );
        }

        // Install a custom `<a>` handler that unwraps image-only links. An
        // "image-only link" is an anchor whose rendered inner content, after
        // trimming, begins with the literal prefix of the configured image
        // placeholder (default `![` when no custom placeholder is set, to
        // match htmd's built-in image rendering).
        //
        // The handler walks the element's children once to inspect the
        // rendered content, which duplicates work the default anchor handler
        // would do for non-image links. Empirically the overhead is small
        // (a few percent on pathological tiers) and the code is easier to
        // audit this way than reimplementing htmd's anchor rendering inline.
        if self.drop_image_only_links {
            let image_prefix: String = self
                .image_placeholder
                .as_deref()
                .map(|t| {
                    // Literal prefix is everything up to the first `{alt}`.
                    t.split("{alt}").next().unwrap_or("").to_string()
                })
                .unwrap_or_else(|| "![".to_string());

            builder = builder.add_handler(
                vec!["a"],
                move |handlers: &dyn Handlers, element: Element| -> Option<HandlerResult> {
                    let inner = handlers.walk_children(element.node);
                    let trimmed = inner.content.trim();
                    let is_image_only = trimmed.is_empty()
                        || (!image_prefix.is_empty() && trimmed.starts_with(&image_prefix));
                    if is_image_only {
                        return Some(HandlerResult::from(inner.content));
                    }
                    handlers.fallback(element)
                },
            );
        }

        builder
    }
}

#[pymethods]
impl PyOptions {
    #[new]
    pub fn new() -> Self {
        let defaults = HtmdOptions::default();

        Self {
            heading_style: match defaults.heading_style {
                HtmdHeadingStyle::Atx => "atx".to_string(),
                HtmdHeadingStyle::Setex => "setex".to_string(),
            },

            hr_style: match defaults.hr_style {
                HtmdHrStyle::Dashes => "dashes".to_string(),
                HtmdHrStyle::Asterisks => "asterisks".to_string(),
                HtmdHrStyle::Underscores => "underscores".to_string(),
            },

            br_style: match defaults.br_style {
                HtmdBrStyle::TwoSpaces => "two_spaces".to_string(),
                HtmdBrStyle::Backslash => "backslash".to_string(),
            },

            link_style: match defaults.link_style {
                HtmdLinkStyle::Inlined => "inlined".to_string(),
                HtmdLinkStyle::Referenced => "referenced".to_string(),
                // Added in htmd 0.5 as an `Inlined` sub-variant that prefers
                // CommonMark autolinks when the link text equals the href.
                // Collapsed to "inlined" for wire compatibility with 0.1's
                // string enum; exposing this as a distinct string is a
                // follow-up decision, not scope for the dep bump.
                HtmdLinkStyle::InlinedPreferAutolinks => "inlined".to_string(),
            },

            link_reference_style: match defaults.link_reference_style {
                HtmdLinkReferenceStyle::Full => "full".to_string(),
                HtmdLinkReferenceStyle::Collapsed => "collapsed".to_string(),
                HtmdLinkReferenceStyle::Shortcut => "shortcut".to_string(),
            },

            code_block_style: match defaults.code_block_style {
                HtmdCodeBlockStyle::Indented => "indented".to_string(),
                HtmdCodeBlockStyle::Fenced => "fenced".to_string(),
            },

            code_block_fence: match defaults.code_block_fence {
                HtmdCodeBlockFence::Tildes => "tildes".to_string(),
                HtmdCodeBlockFence::Backticks => "backticks".to_string(),
            },

            bullet_list_marker: match defaults.bullet_list_marker {
                HtmdBulletListMarker::Asterisk => "asterisk".to_string(),
                HtmdBulletListMarker::Dash => "dash".to_string(),
            },

            preformatted_code: defaults.preformatted_code,

            // Special attributes
            skip_tags: Vec::new(),

            // Text-only handler knobs. Defaults are off so new users get
            // htmd's built-in rendering unchanged; setting any of these
            // switches on the custom handler installation in
            // apply_to_builder.
            image_placeholder: None,
            drop_empty_alt_images: false,
            drop_image_only_links: false,
        }
    }
}
