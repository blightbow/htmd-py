use std::borrow::Cow;
use std::sync::OnceLock;

use pyo3::exceptions::{PyRuntimeError, PyValueError};
use pyo3::prelude::*;

use htmd_lib::convert as htmd_convert;
use htmd_lib::HtmlToMarkdown;

use lol_html::html_content::Element as LolElement;
use lol_html::{rewrite_str, ElementContentHandlers, RewriteStrSettings, Selector};

use regex::Regex;

// Import the Python option classes we defined
mod options;
use options::PyOptions;

/// Convert an HTML string to Markdown, with optional options.
#[pyfunction(signature=(html, options=None))]
fn convert_html(html: &str, options: Option<PyOptions>) -> PyResult<String> {
    if let Some(py_opts) = options {
        let builder = HtmlToMarkdown::builder();
        let builder = py_opts.apply_to_builder(builder);

        let converter = builder.build();
        match converter.convert(html) {
            Ok(markdown) => Ok(markdown),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Conversion error: {}",
                e
            ))),
        }
    } else {
        // Use default options
        match htmd_convert(html) {
            Ok(markdown) => Ok(markdown),
            Err(e) => Err(PyErr::new::<pyo3::exceptions::PyRuntimeError, _>(format!(
                "Conversion error: {}",
                e
            ))),
        }
    }
}

/// Create options configured to skip specific HTML tags during conversion.
#[pyfunction]
fn create_options_with_skip_tags(tags: Vec<String>) -> PyResult<PyOptions> {
    let mut options = PyOptions::new();
    options.skip_tags = tags;
    Ok(options)
}

/// Preprocess HTML by dropping every element matched by a CSS selector.
///
/// Drives lol_html's streaming rewriter with a handler that removes each
/// matching element and its entire subtree. The expected use is to strip
/// noise (navboxes, reference lists, edit links, etc.) before handing the
/// cleaned HTML off to `convert_html`.
///
/// `drop_selectors` is a list of CSS selectors in lol_html's supported
/// subset (tag names, classes, IDs, attribute selectors including
/// `[class*="foo"]`, descendant combinators). Invalid selectors raise
/// `ValueError` before any rewriting work begins.
#[pyfunction]
fn preprocess_selectors(html: &str, drop_selectors: Vec<String>) -> PyResult<String> {
    // Pre-parse every selector so we fail fast with a clean PyValueError
    // rather than panicking inside lol_html's handler setup.
    let parsed: Vec<Selector> = drop_selectors
        .iter()
        .map(|sel| {
            sel.parse::<Selector>().map_err(|e| {
                PyValueError::new_err(format!("invalid CSS selector '{sel}': {e}"))
            })
        })
        .collect::<PyResult<_>>()?;

    let element_handlers: Vec<_> = parsed
        .into_iter()
        .map(|sel| {
            (
                Cow::Owned(sel),
                ElementContentHandlers::default().element(|el: &mut LolElement| {
                    el.remove();
                    Ok(())
                }),
            )
        })
        .collect();

    let settings = RewriteStrSettings {
        element_content_handlers: element_handlers,
        ..RewriteStrSettings::new()
    };

    rewrite_str(html, settings).map_err(|e| {
        PyRuntimeError::new_err(format!("lol_html rewrite failed: {e}"))
    })
}

/// Rewrite citation-link markdown patterns to footnote-marker syntax.
///
/// Specifically matches `[<bracket>N<bracket>](#cite_note-...)` where
/// `<bracket>` is either the backslash-escaped bracket htmd produces
/// (`\[`, `\]`) or the numeric HTML entities some other converters emit
/// (`&#91;`, `&#93;`), and rewrites those matches to `[^N]` footnote
/// markers.
///
/// Useful after converting MediaWiki-style content: htmd renders
/// `<sup class="reference"><a href="#cite_note-foo">[1]</a></sup>` as
/// `[\[1\]](#cite_note-foo)` by default, which downstream markdown parsers
/// don't treat as footnote references. This function normalises them.
#[pyfunction]
fn postprocess_footnote_markers(md: &str) -> String {
    footnote_regex().replace_all(md, "[^$1]").into_owned()
}

fn footnote_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // The inner bracket group alternates htmd's `\[` / `\]` and the
        // HTML numeric entities `&#91;` / `&#93;`. The link target pattern
        // accepts any fragment starting with `#cite_note-`.
        Regex::new(r"\[(?:\\\[|&#91;)(\d+)(?:\\\]|&#93;)\]\(#cite_note-[^)]*\)")
            .expect("footnote regex compile")
    })
}

#[pymodule]
fn htmd(m: &Bound<'_, PyModule>) -> PyResult<()> {
    // Expose the functions
    m.add_function(wrap_pyfunction!(convert_html, m)?)?;
    m.add_function(wrap_pyfunction!(create_options_with_skip_tags, m)?)?;
    m.add_function(wrap_pyfunction!(preprocess_selectors, m)?)?;
    m.add_function(wrap_pyfunction!(postprocess_footnote_markers, m)?)?;

    // Expose the classes
    m.add_class::<PyOptions>()?;

    // Add enum constants for HeadingStyle
    let heading_style = PyModule::new(m.py(), "HeadingStyle")?;
    heading_style.setattr("ATX", "atx")?;
    heading_style.setattr("SETEX", "setex")?;
    m.setattr("HeadingStyle", heading_style)?;

    // Add enum constants for HrStyle
    let hr_style = PyModule::new(m.py(), "HrStyle")?;
    hr_style.setattr("DASHES", "dashes")?;
    hr_style.setattr("ASTERISKS", "asterisks")?;
    hr_style.setattr("UNDERSCORES", "underscores")?;
    m.setattr("HrStyle", hr_style)?;

    // Add enum constants for BrStyle
    let br_style = PyModule::new(m.py(), "BrStyle")?;
    br_style.setattr("TWO_SPACES", "two_spaces")?;
    br_style.setattr("BACKSLASH", "backslash")?;
    m.setattr("BrStyle", br_style)?;

    // Add enum constants for LinkStyle
    let link_style = PyModule::new(m.py(), "LinkStyle")?;
    link_style.setattr("INLINED", "inlined")?;
    link_style.setattr("REFERENCED", "referenced")?;
    m.setattr("LinkStyle", link_style)?;

    // Add enum constants for LinkReferenceStyle
    let link_reference_style = PyModule::new(m.py(), "LinkReferenceStyle")?;
    link_reference_style.setattr("FULL", "full")?;
    link_reference_style.setattr("COLLAPSED", "collapsed")?;
    link_reference_style.setattr("SHORTCUT", "shortcut")?;
    m.setattr("LinkReferenceStyle", link_reference_style)?;

    // Add enum constants for CodeBlockStyle
    let code_block_style = PyModule::new(m.py(), "CodeBlockStyle")?;
    code_block_style.setattr("INDENTED", "indented")?;
    code_block_style.setattr("FENCED", "fenced")?;
    m.setattr("CodeBlockStyle", code_block_style)?;

    // Add enum constants for CodeBlockFence
    let code_block_fence = PyModule::new(m.py(), "CodeBlockFence")?;
    code_block_fence.setattr("TILDES", "tildes")?;
    code_block_fence.setattr("BACKTICKS", "backticks")?;
    m.setattr("CodeBlockFence", code_block_fence)?;

    // Add enum constants for BulletListMarker
    let bullet_list_marker = PyModule::new(m.py(), "BulletListMarker")?;
    bullet_list_marker.setattr("ASTERISK", "asterisk")?;
    bullet_list_marker.setattr("DASH", "dash")?;
    m.setattr("BulletListMarker", bullet_list_marker)?;

    // Explicitly set `__all__` so the enum-style submodules above are
    // re-exported by the autogenerated `from .htmd import *` in the
    // package-level `__init__.py`. PyO3 0.28's `#[pymodule]` attribute
    // auto-generates `__all__` from registered functions and classes only;
    // attributes set via `m.setattr(...)` (which is how the enum constants
    // are wired) would otherwise be absent from `__all__` and invisible
    // at the `htmd` namespace level (`htmd.HeadingStyle`, etc.). This
    // restores the pre-0.28 behavior the README documents.
    m.setattr(
        "__all__",
        vec![
            "convert_html",
            "create_options_with_skip_tags",
            "preprocess_selectors",
            "postprocess_footnote_markers",
            "Options",
            "HeadingStyle",
            "HrStyle",
            "BrStyle",
            "LinkStyle",
            "LinkReferenceStyle",
            "CodeBlockStyle",
            "CodeBlockFence",
            "BulletListMarker",
        ],
    )?;

    Ok(())
}
