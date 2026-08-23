//! CSS contract tests for `style.css` (which @imports from `css/`).
//!
//! These tests don't run a browser — they assert that specific selectors are
//! present in the stylesheet so that visual contracts depended on by the
//! Leptos templates (e.g. `dashboard.rs`'s "Active Models" section) can't be
//! silently dropped.
//!
//! Each test reads all CSS partials from the `css/` directory via `include_str!`
//! and concatenates them in import order so it runs as a normal Rust integration
//! test (`cargo test --package tama-web`) without needing a WASM toolchain.

// Import all CSS partials in the same order as style.css @imports.
const CSS_01: &str = include_str!("../css/01-custom-properties.css");
const CSS_02: &str = include_str!("../css/02-reset-base.css");
const CSS_03: &str = include_str!("../css/03-layout.css");
const CSS_04: &str = include_str!("../css/04-cards-grid-tables.css");
const CSS_05: &str = include_str!("../css/05-buttons-forms-progress.css");
const CSS_06: &str = include_str!("../css/06-badges-list-card.css");
const CSS_07: &str = include_str!("../css/07-gauges-charts.css");
const CSS_08: &str = include_str!("../css/08-updates.css");
const CSS_09: &str = include_str!("../css/09-utilities.css");
const CSS_10: &str = include_str!("../css/10-page-components.css");
const CSS_11: &str = include_str!("../css/11-models.css");
const CSS_12: &str = include_str!("../css/12-forms-alerts.css");
const CSS_13: &str = include_str!("../css/13-downloads.css");
const CSS_14: &str = include_str!("../css/14-model-editor.css");
const CSS_15: &str = include_str!("../css/15-dashboard.css");
const CSS_16: &str = include_str!("../css/16-benchmarks.css");
const CSS_17: &str = include_str!("../css/17-api-docs.css");
const CSS_18: &str = include_str!("../css/18-aliases.css");
const CSS_19: &str = include_str!("../css/19-gpu-device-card.css");
const CSS_20: &str = include_str!("../css/20-api-keys.css");
const CSS_21: &str = include_str!("../css/21-dashboard-hosts.css");

/// Concatenate all CSS partials in import order.
fn combined_css() -> String {
    [
        CSS_01, CSS_02, CSS_03, CSS_04, CSS_05, CSS_06, CSS_07, CSS_08, CSS_09, CSS_10, CSS_11,
        CSS_12, CSS_13, CSS_14, CSS_15, CSS_16, CSS_17, CSS_18, CSS_19, CSS_20, CSS_21,
    ]
    .join("\n")
}

/// Strip C-style block comments (`/* ... */`) from a CSS source. We use this
/// so that selector-presence assertions can't be satisfied accidentally by
/// commented-out rules.
fn strip_css_comments(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'/' && bytes[i + 1] == b'*' {
            // Skip until matching `*/`
            i += 2;
            while i + 1 < bytes.len() && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(bytes.len());
        } else {
            out.push(bytes[i] as char);
            i += 1;
        }
    }
    out
}

/// Find a CSS rule block (the `{ ... }` body) for the given selector. Returns
/// `None` if the selector doesn't appear at the top level. Selector matching
/// is whitespace-insensitive at the boundary so `.foo .bar` matches both
/// `.foo .bar {` and `.foo  .bar {`.
fn rule_body<'a>(css: &'a str, selector: &str) -> Option<&'a str> {
    // Split into selector groups separated by `{`. We then check each preceding
    // chunk for an exact (trimmed) match against `selector`.
    let mut search_from = 0usize;
    while let Some(brace) = css[search_from..].find('{') {
        let abs_brace = search_from + brace;
        // Walk backwards from `abs_brace` to find the start of the selector
        // (after the previous `}` or start-of-file).
        let sel_start = css[..abs_brace].rfind('}').map(|p| p + 1).unwrap_or(0);
        let raw_selector = css[sel_start..abs_brace].trim();
        // Compare normalised whitespace.
        let normalised: String = raw_selector
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let target: String = selector.split_whitespace().collect::<Vec<_>>().join(" ");
        if normalised == target {
            // Find the matching closing brace.
            let body_start = abs_brace + 1;
            let mut depth = 1i32;
            let mut idx = body_start;
            while idx < css.len() && depth > 0 {
                let c = css.as_bytes()[idx];
                if c == b'{' {
                    depth += 1;
                } else if c == b'}' {
                    depth -= 1;
                }
                idx += 1;
            }
            if depth == 0 {
                // idx is one past the closing brace.
                return Some(&css[body_start..idx - 1]);
            }
            return None;
        }
        search_from = abs_brace + 1;
    }
    None
}

/// The dashboard's Hosts `<section>` (which now also contains the merged
/// active-models groups) is wrapped in a `.dashboard-hosts` class. The CSS
/// must give that section vertical breathing room so it doesn't visually
/// collide with the content directly above it.
#[test]
fn test_style_css_defines_dashboard_hosts_section_spacing() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".dashboard-hosts")
        .expect("style.css must define a `.dashboard-hosts` rule");
    assert!(
        body.contains("margin-top"),
        "`.dashboard-hosts` rule must set `margin-top` to separate the section from the content above; got: {body}"
    );
}

/// Inside `.dashboard-hosts` we render a `.page-header` row containing the
/// section title and the host/model summary count. It needs its own bottom
/// margin so the header doesn't sit flush against the host cards grid.
#[test]
fn test_style_css_defines_dashboard_hosts_page_header_spacing() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".dashboard-hosts .page-header")
        .expect("style.css must define a `.dashboard-hosts .page-header` rule");
    assert!(
        body.contains("margin-bottom"),
        "`.dashboard-hosts .page-header` rule must set `margin-bottom` to separate the header from the grid; got: {body}"
    );
}

/// The host card renders CPU + RAM on one compact line inside a
/// `.host-metrics` flex container; it must be a wrapping flex row so the two
/// half-column metric groups stack on narrow cards instead of clipping.
#[test]
fn test_style_css_defines_host_metrics_compact_row() {
    let css = strip_css_comments(&combined_css());
    let body =
        rule_body(&css, ".host-metrics").expect("style.css must define a `.host-metrics` rule");
    assert!(
        body.contains("display: flex"),
        "`.host-metrics` must be a flex row; got: {body}"
    );
    assert!(
        body.contains("flex-wrap: wrap"),
        "`.host-metrics` must wrap to stacked on narrow cards; got: {body}"
    );
}

/// Sanity-check the helper: it must locate top-level rules and ignore
/// commented-out copies. This guards against false positives in the two
/// dashboard-section assertions above.
#[test]
fn test_rule_body_finds_top_level_rules_and_ignores_comments() {
    let css = strip_css_comments(
        "/* .foo { margin-top: 1rem; } */\n.foo { margin-top: 2rem; }\n.bar .baz { margin-bottom: 0.5rem; }",
    );
    let foo = rule_body(&css, ".foo").expect("`.foo` rule should be found");
    assert!(foo.contains("margin-top: 2rem"));
    assert!(!foo.contains("1rem"), "commented-out copy must be stripped");

    let bar_baz = rule_body(&css, ".bar .baz").expect("`.bar .baz` rule should be found");
    assert!(bar_baz.contains("margin-bottom: 0.5rem"));

    assert!(rule_body(&css, ".missing").is_none());
}

/// The `.model-section` container wraps each section of model cards.
/// It needs vertical spacing (`margin-bottom`) to separate sections visually.
#[test]
fn test_style_css_defines_model_section_spacing() {
    let css = strip_css_comments(&combined_css());
    let body =
        rule_body(&css, ".model-section").expect("style.css must define a `.model-section` rule");
    assert!(
        body.contains("margin-bottom"),
        "`.model-section` rule must set `margin-bottom` to separate sections; got: {body}"
    );
}

/// The last `.model-section` should not have extra bottom margin.
#[test]
fn test_style_css_defines_model_section_last_child_spacing() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".model-section:last-child")
        .expect("style.css must define a `.model-section:last-child` rule");
    assert!(
        body.contains("margin-bottom: 0"),
        "`.model-section:last-child` rule must set `margin-bottom: 0`; got: {body}"
    );
}

/// The `.model-section__title` element styles the section header.
/// It needs appropriate typography and a bottom border for visual separation.
#[test]
fn test_style_css_defines_model_section_title_styling() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".model-section__title")
        .expect("style.css must define a `.model-section__title` rule");
    assert!(
        body.contains("font-size")
            && body.contains("font-weight")
            && body.contains("border-bottom")
            && body.contains("padding-bottom"),
        "`.model-section__title` rule must set typography and border styles; got: {body}"
    );
}

/// The `.checkbox-label` class is used throughout the config editor for
/// boolean toggles (e.g. `api_keys_enabled`, `oauth2.enabled`). It must
/// render the label and checkbox inline so they sit on a single row.
#[test]
fn test_style_css_defines_checkbox_label() {
    let css = strip_css_comments(&combined_css());
    let body =
        rule_body(&css, ".checkbox-label").expect("style.css must define a `.checkbox-label` rule");
    assert!(
        body.contains("display: inline-flex") || body.contains("display:inline-flex"),
        "`.checkbox-label` must use `display: inline-flex` to lay out the label and checkbox in a row; got: {body}"
    );
}

/// The `.form-subsection` class wraps a sub-group of form fields (e.g. the
/// OAuth2 provider config inside the proxy form). It must give the subsection
/// a visible boundary so it reads as a distinct sub-section.
#[test]
fn test_style_css_defines_form_subsection() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".form-subsection")
        .expect("style.css must define a `.form-subsection` rule");
    assert!(
        body.contains("border") && body.contains("padding"),
        "`.form-subsection` must have a border and padding to visually separate it from the surrounding form; got: {body}"
    );
    // The legend inside the fieldset should be styled to look like a section
    // heading, not the browser's default.
    let legend = rule_body(&css, ".form-subsection legend")
        .expect("style.css must define a `.form-subsection legend` rule");
    assert!(
        legend.contains("font-size") && legend.contains("font-weight"),
        "`.form-subsection legend` must have typography styling; got: {legend}"
    );
}

/// The GPU utilization + VRAM metric lines must be a consistent 3-zone
/// grid — fixed label column, fixed right-aligned value column, and the
/// flex-1 bar track — so the label/value/bar columns line up across
/// adjacent GPU tiles in the 2-column GPU grid.
#[test]
fn test_style_css_defines_gpu_util_line() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".host-gpu-row__util-line")
        .expect("style.css must define a `.host-gpu-row__util-line` rule");
    assert!(
        body.contains("display: grid")
            && body.contains("grid-template-columns")
            && body.contains("3.8ch")
            && body.contains("4.5ch")
            && body.contains("1fr")
            && body.contains("align-items: center"),
        "`.host-gpu-row__util-line` must be a 3-zone grid (label | value | bar); got: {body}"
    );
}

/// The small `util` label in the utilization line must be visibly quieter
/// than the row title: a muted color and a smaller font size.
#[test]
fn test_style_css_defines_gpu_util_label() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".host-gpu-row__metric-label")
        .expect("style.css must define a `.host-gpu-row__metric-label` rule");
    assert!(
        body.contains("color") && body.contains("font-size"),
        "`.host-gpu-row__metric-label` must style the label color and size; got: {body}"
    );
}

/// The right-aligned % value in the utilization line must use tabular
/// numerals so digits don't jitter as utilization ticks up.
#[test]
fn test_style_css_defines_gpu_util_value() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".host-gpu-row__metric-value")
        .expect("style.css must define a `.host-gpu-row__metric-value` rule");
    assert!(
        body.contains("tabular-nums"),
        "`.host-gpu-row__metric-value` must use tabular numerals; got: {body}"
    );
}

/// Every bar in a host card (CPU, RAM, GPU utilization, GPU VRAM) shares
/// the same `.host-gpu-row__util-bar` track — a 6px track with a full-
/// height fill — so all bars have identical height and the GPU rows read
/// as a clean grid.
#[test]
fn test_style_css_keeps_host_metric_bars_uniform() {
    let css = strip_css_comments(&combined_css());
    let track = rule_body(&css, ".host-gpu-row__util-bar")
        .expect("style.css must define a `.host-gpu-row__util-bar` rule");
    assert!(
        track.contains("height: 6px"),
        "`.host-gpu-row__util-bar` must keep the 6px track height; got: {track}"
    );
    let fill = rule_body(&css, ".host-gpu-row__util-bar .progress-bar-fill")
        .expect("style.css must define a `.host-gpu-row__util-bar .progress-bar-fill` rule");
    assert!(
        fill.contains("height: 100%") && fill.contains("background"),
        "bar fill must be full-height with a background color; got: {fill}"
    );
}

/// Extract the balanced `{ ... }` body of the media block whose query
/// starts with `query` — `rule_body` only matches plain top-level
/// selectors, so media-query rules need their own lookup.
fn media_block<'a>(css: &'a str, query: &str) -> Option<&'a str> {
    let start = css.find(query)?;
    let brace = css[start..].find('{')? + start;
    let mut depth = 1i32;
    let mut idx = brace + 1;
    while idx < css.len() && depth > 0 {
        match css.as_bytes()[idx] {
            b'{' => depth += 1,
            b'}' => depth -= 1,
            _ => {}
        }
        idx += 1;
    }
    Some(&css[brace + 1..idx - 1])
}

/// The GPU section's tiles must sit in a responsive grid: one column by
/// default (mobile / narrow cards), two columns from 720px viewport width
/// up so two GPUs sit side by side (four GPUs = 2×2).
#[test]
fn test_style_css_gpu_grid_two_columns_from_720px() {
    let css = strip_css_comments(&combined_css());
    let block = media_block(&css, "@media (min-width: 720px)")
        .expect("a `@media (min-width: 720px)` rule for the GPU grid");
    assert!(
        block.contains(".host-card__gpu-grid"),
        "the 720px media rule must target `.host-card__gpu-grid`; got: {block}"
    );
    assert!(
        block.contains("repeat(2, minmax(0, 1fr))"),
        "the 720px media rule must set the grid to two equal 1fr columns; got: {block}"
    );
}

/// Each GPU tile must carry the project's neutral 1px border (the same
/// treatment `.card` uses), a small radius, and tight padding — so the
/// two-across grid reads as connected tiles instead of stacked dividers.
#[test]
fn test_style_css_gpu_tile_border() {
    let css = strip_css_comments(&combined_css());
    let body = rule_body(&css, ".host-card__gpu-grid .host-gpu-row")
        .expect("style.css must define a `.host-card__gpu-grid .host-gpu-row` tile rule");
    assert!(
        body.contains("border: 1px solid var(--border-color)"),
        "the GPU tile must reuse the project's neutral 1px border; got: {body}"
    );
    assert!(
        body.contains("border-radius: 8px") && body.contains("padding: 8px 10px"),
        "the GPU tile must have an 8px radius and 8px 10px padding; got: {body}"
    );
}

/// On narrow widths (below 720px) the GPU meta summary must wrap to its
/// own line under the name instead of truncating the GPU name to
/// ellipsis.
#[test]
fn test_style_css_gpu_meta_wraps_to_own_line_on_mobile() {
    let css = strip_css_comments(&combined_css());
    let block = media_block(&css, "@media (max-width: 719.98px)")
        .expect("a mobile media rule for the GPU meta wrap");
    assert!(
        block.contains(".host-card__gpu-grid .host-gpu-row__meta"),
        "the mobile rule must target the grid's meta span; got: {block}"
    );
    assert!(
        block.contains("flex-basis: 100%") && block.contains("margin-left: 0"),
        "the mobile meta must take a full line under the name; got: {block}"
    );
}
