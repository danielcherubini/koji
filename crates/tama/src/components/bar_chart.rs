use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::utils::chart_utils::{format_duration_label, format_relative_time};

/// One data series for a [`BarChart`]. Multiple series render as paired
/// side-by-side bars within each bucket (e.g. per-GPU utilization).
#[derive(Debug, Clone)]
pub struct ChartSeries {
    /// Label shown in the tooltip (e.g. "GPU 0").
    pub label: String,
    /// CSS color string (e.g. `"var(--accent-blue)"`).
    pub color: String,
    /// One value per bucket. Must match `timestamps.len()` when timestamps are
    /// provided; otherwise one bar per value.
    pub data: Vec<f32>,
}

/// Compute the CSS fill-opacity for a bar based on its value relative to max.
/// Returns a value in [0.25, 1.0].
fn compute_opacity(value: f32, safe_max: f32) -> f32 {
    (0.25 + 0.75 * (value / safe_max)).clamp(0.25, 1.0)
}

/// Round an observed max value up to a "nice" number (1-2-5-10 sequence) with
/// ~10% headroom. Used for auto-scaling the network chart so the Y-axis stays
/// stable across 2s updates instead of jittering with every new data point.
///
/// Returns the smallest value from the [1, 2, 5, 10, 20, 50, ...] sequence that
/// is >= `observed * 1.1`, with a floor of 1.0. The 10% headroom absorbs per-tick
/// noise so the scale only steps to a new clean value when the data genuinely
/// moves past a nice boundary (preventing jitter near boundaries like 9.5 vs 10.5
/// both rounding to 10).
fn nice_max(observed: f32) -> f32 {
    const NICE: [f32; 15] = [
        1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0,
        20000.0, 50000.0,
    ];
    let target = (observed * 1.1).max(1.0);
    for &n in NICE.iter() {
        if n >= target {
            return n;
        }
    }
    // Above the table — scale up in 2x steps
    let mut n = 50000.0_f32;
    while n < target {
        n *= 2.0;
    }
    n
}

/// Compute the hover opacity (base + 0.15, capped at 1.0).
fn compute_hover_opacity(value: f32, safe_max: f32) -> f32 {
    (compute_opacity(value, safe_max) + 0.15).clamp(0.25, 1.0)
}

/// Compute the x-position percentage for a given bucket index.
/// Centers the position on the bar.
fn bucket_x_pct(index: usize, num_buckets: usize) -> f32 {
    if num_buckets == 0 {
        return 50.0;
    }
    if num_buckets == 1 {
        return 50.0;
    }
    (index as f32 + 0.5) * (100.0 / num_buckets as f32)
}

/// A responsive SVG bar chart component for displaying time-series data.
///
/// Renders vertical bars with opacity proportional to value, interactive
/// hover highlighting, and time axis labels. Two input modes:
///
/// - **Single/dual series** (default): pass `data` (and optionally `data2` +
///   `color`/`color2`). Used by the CPU and Memory cards.
/// - **N series**: pass `series: Vec<ChartSeries>`. Renders N paired bars per
///   bucket, N-way hover highlight, and a tooltip listing all N values
///   color-coded. Used by the GPU card (one series per GPU device).
///
/// When `series` is non-empty it takes precedence over `data`/`data2`.
///
/// ## Parameters
///
/// * `data` — Primary values to plot (one bar per value).
/// * `max_value` — Y-axis ceiling. > 0 = fixed (CPU = 100, memory = RAM total);
///   <= 0 = auto-scale to a stable "nice" number (network-style).
/// * `color` / `color2` — CSS colors for the primary/secondary single-series bars.
/// * `height` — SVG height in pixels. Recommended range: 30–150.
/// * `timestamps` — Optional Unix ms timestamps for each bar. Enables time
///   axis labels (`-15m` / `now`) and bucket-relative tooltip times.
/// * `unit_label` — Unit string shown in tooltip (e.g. `"%"`, `"MiB"`).
/// * `data2` / `color2` — Optional secondary series for paired single-mode bars.
/// * `series` — Optional N-series list. When non-empty, overrides the above.
#[component]
pub fn BarChart(
    #[prop(default = Vec::new())] data: Vec<f32>,
    max_value: f32,
    color: String,
    height: f32,
    #[prop(default = Vec::new())] timestamps: Vec<i64>,
    #[prop(default = String::new())] unit_label: String,
    #[prop(default = Vec::new())] data2: Vec<f32>,
    #[prop(default = String::new())] color2: String,
    #[prop(default = Vec::new())] series: Vec<ChartSeries>,
) -> impl IntoView {
    let hover = RwSignal::new(None::<usize>);

    // Normalize to an internal series representation so the rest of the
    // component has a single code path. The `series` prop wins when non-empty;
    // otherwise we synthesize 1 (or 2, when data2 is present) series from the
    // legacy data/data2/color/color2 props.
    let use_series: Vec<ChartSeries> = if !series.is_empty() {
        series
    } else {
        let mut v = Vec::with_capacity(2);
        v.push(ChartSeries {
            label: String::new(),
            color: color.clone(),
            data: data.clone(),
        });
        if !data2.is_empty() {
            v.push(ChartSeries {
                label: String::new(),
                color: color2.clone(),
                data: data2.clone(),
            });
        }
        v
    };

    let num_series = use_series.len();
    let num_buckets = use_series.first().map(|s| s.data.len()).unwrap_or(0);
    let has_data = num_buckets > 0 && num_series > 0;
    let timestamps_valid = !timestamps.is_empty() && timestamps.len() == num_buckets;

    // Determine the Y-axis scale. Fixed when max_value > 0 (CPU = 100, memory =
    // RAM total); otherwise auto-scale to a stable "nice" number from the
    // observed max across all series. The 10% headroom in `nice_max` absorbs
    // per-tick noise so the scale only steps to a clean value when the data
    // genuinely crosses a nice boundary.
    let safe_max = if max_value > 0.0 {
        max_value
    } else {
        let observed = use_series
            .iter()
            .flat_map(|s| s.data.iter().copied())
            .fold(0.0_f32, f32::max);
        nice_max(observed).max(1.0)
    };

    // Store series + unit in signals for reactive access in closures.
    let series_signal = RwSignal::new(use_series);
    let unit_label_signal = RwSignal::new(unit_label);

    // Compute time axis labels
    let (left_label, right_label) = if timestamps_valid && !timestamps.is_empty() {
        let oldest_ts = *timestamps.first().unwrap();
        let now_ms = js_sys::Date::now() as i64;
        let diff_secs = (now_ms - oldest_ts) / 1_000;
        (format_duration_label(diff_secs.max(0)), "now".to_string())
    } else {
        (String::new(), String::new())
    };

    // Helper to find nearest bucket from mouse X position
    let find_nearest_bucket = move |x_pct: f32| -> usize {
        if num_buckets == 0 {
            return 0;
        }
        if num_buckets == 1 {
            return 0;
        }
        let slot_width = 100.0 / num_buckets as f32;
        let raw_index = ((x_pct / slot_width) - 0.5).round().max(0.0) as usize;
        raw_index.clamp(0, num_buckets - 1)
    };

    let on_mouse_move = move |ev: leptos::ev::MouseEvent| {
        if !has_data {
            hover.set(None);
            return;
        }

        // Use current_target (the <svg> the listener is attached to) rather
        // than target (the element under the cursor, e.g. a <rect> bar).
        // Casting a rect to SvgsvgElement fails, which previously caused
        // hover to only activate on the bar perimeter/gaps.
        let target = match ev.current_target() {
            Some(t) => t,
            None => {
                hover.set(None);
                return;
            }
        };
        let svg_el: web_sys::SvgsvgElement = match target.dyn_into() {
            Ok(el) => el,
            Err(_) => {
                hover.set(None);
                return;
            }
        };

        let rect = svg_el.get_bounding_client_rect();
        let svg_width = rect.width();
        if svg_width <= 0.0 {
            hover.set(None);
            return;
        }

        let mouse_x = ev.client_x() as f64 - rect.left();
        let x_pct = (((mouse_x / svg_width) * 100.0) as f32).clamp(0.0, 100.0);
        hover.set(Some(find_nearest_bucket(x_pct)));
    };

    let on_mouse_leave = move |_ev: leptos::ev::MouseEvent| {
        hover.set(None);
    };

    // Render bars. For N series, each bucket's slot is divided into N equal
    // sub-slots (minus a small gap between pairs); single-series buckets use
    // the full slot with side padding.
    let bars = move || {
        if !has_data {
            return ().into_any();
        }

        let series = series_signal.get();
        let slot_width = 100.0 / num_buckets as f32;

        let rects: Vec<AnyView> = series
            .iter()
            .enumerate()
            .flat_map(|(s_idx, s)| {
                let color = s.color.clone();
                s.data.iter().enumerate().map(move |(i, &val)| {
                    let bar_height = (val / safe_max * height).max(1.0).clamp(1.0, height);
                    let bar_y = (height - bar_height).clamp(0.0, height);
                    let base_opacity = compute_opacity(val, safe_max);

                    let (bar_width, bar_x) = if num_series == 1 {
                        let gap = slot_width * 0.15;
                        let w = (slot_width - gap * 2.0).max(1.0);
                        (w, (i as f32 * slot_width) + gap)
                    } else {
                        let sub_slot = slot_width / num_series as f32;
                        let gap = 1.0_f32.max(sub_slot * 0.1);
                        let w = (sub_slot - gap).max(1.0);
                        let x = (i as f32 * slot_width) + (s_idx as f32 * sub_slot) + (gap / 2.0);
                        (w, x)
                    };

                    let is_hovered = hover.get().map(|idx| idx == i).unwrap_or(false);
                    let opacity = if is_hovered {
                        compute_hover_opacity(val, safe_max)
                    } else {
                        base_opacity
                    };

                    let fill = color.clone();
                    view! {
                        <rect
                            x=bar_x
                            y=bar_y
                            width=bar_width
                            height=bar_height
                            fill=fill
                            fill-opacity=opacity
                            rx="2"
                            class="bar-rect"
                        />
                    }
                    .into_any()
                })
            })
            .collect();

        view! { {rects} }.into_any()
    };

    // Tooltip HTML element. Lists every series' value at the hovered bucket,
    // color-coded. When a series has a label, it's shown before the value.
    let tooltip_html = move || {
        hover.get().map(|idx| {
            let series = series_signal.get();
            let unit = unit_label_signal.get();
            let x_pct = bucket_x_pct(idx, num_buckets);
            let left_style = format!("left: {}%;", x_pct);

            // Get timestamp for this bucket
            let ts = if timestamps_valid && idx < timestamps.len() {
                Some(timestamps[idx])
            } else if timestamps_valid {
                let oldest = *timestamps.first().unwrap_or(&0);
                Some(oldest + (idx as i64) * 30_000)
            } else {
                None
            };
            let time_str = ts.map(format_relative_time).unwrap_or_default();

            let value_spans: Vec<AnyView> = series
                .iter()
                .filter_map(|s| {
                    if idx >= s.data.len() {
                        return None;
                    }
                    let val = s.data[idx];
                    let color = s.color.clone();
                    let label = s.label.clone();
                    let unit_part = if unit.is_empty() { "" } else { &unit };
                    let text = if label.is_empty() {
                        format!("{:.1}{}", val, unit_part)
                    } else {
                        format!("{} {:.1}{}", label, val, unit_part)
                    };
                    Some(
                        view! {
                            <span class="sparkline-tooltip-value" style=format!("color: {}", color)>
                                {text}
                            </span>
                        }
                        .into_any(),
                    )
                })
                .collect();

            view! {
                <div class="sparkline-tooltip" style=left_style>
                    {value_spans}
                    {if time_str.is_empty() {
                        ().into_any()
                    } else {
                        view! {
                            <span class="sparkline-tooltip-time">{time_str}</span>
                        }.into_any()
                    }}
                </div>
            }
            .into_any()
        })
    };

    view! {
        <div class="sparkline-container">
            <svg
                viewBox=format!("0 0 100 {height}")
                width="100%"
                height="100%"
                class="sparkline"
                preserveAspectRatio="none"
                on:mousemove=on_mouse_move
                on:mouseleave=on_mouse_leave
            >
                {bars}
            </svg>
            // Tooltip HTML element positioned absolutely above the SVG
            {tooltip_html}
            // Time axis labels
            {if !left_label.is_empty() || !right_label.is_empty() {
                view! {
                    <div class="sparkline-time-axis">
                        <span>{left_label}</span>
                        <span>{right_label}</span>
                    </div>
                }.into_any()
            } else {
                ().into_any()
            }}
        </div>
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_opacity_min() {
        // Value = 0 should give min opacity of 0.25
        let opacity = compute_opacity(0.0, 100.0);
        assert!((opacity - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_compute_opacity_max() {
        // Value = safe_max should give opacity of 1.0
        let opacity = compute_opacity(100.0, 100.0);
        assert!((opacity - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_opacity_mid() {
        // Value = 50% of max: 0.25 + 0.75 * 0.5 = 0.625
        let opacity = compute_opacity(50.0, 100.0);
        assert!((opacity - 0.625).abs() < 0.01);
    }

    #[test]
    fn test_compute_opacity_clamped_to_max() {
        // Value > safe_max should still clamp to 1.0
        let opacity = compute_opacity(200.0, 100.0);
        assert!((opacity - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_compute_hover_opacity() {
        let base = compute_opacity(50.0, 100.0); // 0.625
        let hover = compute_hover_opacity(50.0, 100.0);
        assert!((hover - (base + 0.15)).abs() < 0.01);
    }

    #[test]
    fn test_compute_hover_opacity_capped_at_1() {
        // High value: base is 1.0, hover should still be 1.0
        let hover = compute_hover_opacity(100.0, 100.0);
        assert!((hover - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_safe_max_guard() {
        // max_value = 0 should use safe_max = 1.0
        // This prevents division by zero
        let safe_max = 0.0_f32.max(1.0);
        assert_eq!(safe_max, 1.0);
        let opacity = compute_opacity(0.5, safe_max);
        assert!((opacity - 0.625).abs() < 0.01);
    }

    #[test]
    fn test_bucket_x_pct_single_bucket() {
        assert!((bucket_x_pct(0, 1) - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_bucket_x_pct_two_buckets() {
        // Two buckets: each slot is 50% wide, centers at 25% and 75%
        assert!((bucket_x_pct(0, 2) - 25.0).abs() < 0.01);
        assert!((bucket_x_pct(1, 2) - 75.0).abs() < 0.01);
    }

    #[test]
    fn test_bucket_x_pct_multiple_buckets() {
        // 3 buckets: each slot is 33.33% wide
        // Bucket 0 center: (0 + 0.5) * 33.33 = 16.67
        let x0 = bucket_x_pct(0, 3);
        assert!((x0 - 16.67).abs() < 0.1);
        // Bucket 1 center: (1 + 0.5) * 33.33 = 50.0
        let x1 = bucket_x_pct(1, 3);
        assert!((x1 - 50.0).abs() < 0.01);
        // Bucket 2 center: (2 + 0.5) * 33.33 = 83.33
        let x2 = bucket_x_pct(2, 3);
        assert!((x2 - 83.33).abs() < 0.1);
    }

    #[test]
    fn test_bucket_x_pct_zero_buckets() {
        assert!((bucket_x_pct(0, 0) - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_nice_max_floor() {
        // Zero/negative observed → floor of 1.0
        assert!((nice_max(0.0) - 1.0).abs() < 0.01);
        assert!((nice_max(-5.0) - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_nice_max_rounds_up_with_headroom() {
        // 9.0 * 1.1 = 9.9 → next nice >= 9.9 is 10
        assert!((nice_max(9.0) - 10.0).abs() < 0.01);
        // 10.0 * 1.1 = 11.0 → next nice >= 11.0 is 20
        assert!((nice_max(10.0) - 20.0).abs() < 0.01);
        // 18.0 * 1.1 = 19.8 → 20
        assert!((nice_max(18.0) - 20.0).abs() < 0.01);
        // 20.0 * 1.1 = 22.0 → 50
        assert!((nice_max(20.0) - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_nice_max_no_boundary_jitter() {
        // Values straddling the 10 boundary should BOTH round to 20 (not 10),
        // preventing per-tick scale flips when data hovers near a nice number.
        assert!((nice_max(9.5) - 20.0).abs() < 0.01);
        assert!((nice_max(10.5) - 20.0).abs() < 0.01);
    }

    #[test]
    fn test_nice_max_large_values() {
        // 100 * 1.1 = 110 → 200
        assert!((nice_max(100.0) - 200.0).abs() < 0.01);
        // Above the table scales in 2x steps: 60000 * 1.1 = 66000 → 100000
        assert!((nice_max(60000.0) - 100000.0).abs() < 0.01);
    }
}
