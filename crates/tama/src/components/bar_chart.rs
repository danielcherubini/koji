use leptos::prelude::*;
use wasm_bindgen::JsCast;

use crate::utils::chart_utils::{format_duration_label, format_relative_time};

/// Bucket raw data into 30-second windows using timestamps.
///
/// If `timestamps` is provided and valid (non-empty, same length as `data`),
/// groups data points into 30-second buckets starting from the oldest timestamp.
/// Each bucket collects all points whose timestamp falls within
/// `[bucket_start, bucket_start + 30000ms)` and computes the average.
/// Empty buckets are skipped.
///
/// If `timestamps` is empty or length doesn't match, returns `data` unchanged
/// (each point becomes its own bar).
fn bucket_data(data: &[f32], timestamps: &[i64]) -> Vec<f32> {
    if timestamps.is_empty() || timestamps.len() != data.len() {
        return data.to_vec();
    }

    let oldest = *timestamps.iter().min().unwrap_or(&0);
    let bucket_size_ms = 30_000i64;

    // Collect (bucket_index, sum, count) for each bucket
    let mut buckets: std::collections::HashMap<usize, (f64, usize)> =
        std::collections::HashMap::new();

    for (ts, &val) in timestamps.iter().zip(data.iter()) {
        let bucket_idx = ((ts - oldest) / bucket_size_ms) as usize;
        let entry = buckets.entry(bucket_idx).or_insert((0.0, 0));
        entry.0 += val as f64;
        entry.1 += 1;
    }

    // Sort by bucket index and compute averages, skipping empty buckets
    let mut sorted_keys: Vec<usize> = buckets.keys().cloned().collect();
    sorted_keys.sort();

    sorted_keys
        .into_iter()
        .filter_map(|k| {
            let (sum, count) = buckets[&k];
            if count > 0 {
                Some((sum / count as f64) as f32)
            } else {
                None
            }
        })
        .collect()
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
/// hover highlighting, and time axis labels. Supports paired (dual) data
/// sets rendered side-by-side.
///
/// ## Parameters
///
/// * `data` — Sample values to plot. Each value is expected to be in
///   the range `[0, max_value]`. Empty vectors render an empty chart.
/// * `max_value` — Maximum expected Y-axis value. Must be > 0; defaults to 1.0.
/// * `color` — CSS color string for the chart (e.g. `"var(--accent-green)"`).
/// * `height` — SVG height in pixels. Recommended range: 30–150.
/// * `timestamps` — Optional Unix ms timestamps for each data point. If provided
///   and matching `data.len()`, enables 30-second bucketing and time axis labels.
/// * `unit_label` — Unit string shown in tooltip (e.g. `"%"`, `"MiB"`).
/// * `data2` — Optional secondary data set for paired bar rendering.
/// * `color2` — CSS color string for the secondary data set.
#[component]
pub fn BarChart(
    data: Vec<f32>,
    max_value: f32,
    color: String,
    height: f32,
    #[prop(default = Vec::new())] timestamps: Vec<i64>,
    #[prop(default = String::new())] unit_label: String,
    #[prop(default = Vec::new())] data2: Vec<f32>,
    #[prop(default = String::new())] color2: String,
) -> impl IntoView {
    let hover = RwSignal::new(None::<(usize, Option<usize>)>);

    // Bucket data into 30-second windows
    let bucketed = bucket_data(&data, &timestamps);
    let bucketed2 = if !data2.is_empty() {
        bucket_data(&data2, &timestamps)
    } else {
        Vec::new()
    };

    // Determine the Y-axis scale. When the caller passes a fixed max_value > 0
    // (CPU = 100, memory = RAM total), use it directly so the scale is
    // meaningful regardless of observed values. When max_value <= 0 (network
    // has no natural ceiling), auto-scale to a stable "nice" number derived from
    // the bucketed data. The 10% headroom in `nice_max` absorbs per-tick noise so
    // the scale doesn't jitter every 2s — it only steps to a clean value when the
    // data genuinely moves past a nice boundary.
    let safe_max = if max_value > 0.0 {
        max_value
    } else {
        let observed = bucketed
            .iter()
            .chain(bucketed2.iter())
            .copied()
            .fold(0.0_f32, f32::max);
        nice_max(observed).max(1.0)
    };

    let num_buckets = bucketed.len();
    let has_data = num_buckets > 0;
    let timestamps_valid = !timestamps.is_empty() && timestamps.len() == data.len();
    let has_paired = !bucketed2.is_empty();

    // Store values in signals for reactive access in closures
    let bucketed_signal = RwSignal::new(bucketed.clone());
    let bucketed2_signal = RwSignal::new(bucketed2.clone());
    let color_signal = RwSignal::new(color);
    let color2_signal = RwSignal::new(color2);
    let unit_label_signal = RwSignal::new(unit_label);

    // Compute time axis labels
    let (left_label, right_label) = if timestamps_valid && !timestamps.is_empty() {
        let oldest_ts = *timestamps.first().unwrap();
        let now_ms = js_sys::Date::now() as i64;
        let diff_secs = (now_ms - oldest_ts) / 1_000;
        let left = format_duration_label(diff_secs.max(0));
        let right = "now".to_string();
        (left, right)
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

        // Use current_target (the <svg> element the listener is attached to)
        // rather than target (the actual element under the cursor, e.g. a
        // <rect> bar). Casting a rect to SvgsvgElement fails, which previously
        // caused hover to only activate on the bar perimeter/gaps.
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
        let x_pct = ((mouse_x / svg_width) * 100.0) as f32;
        let x_pct = x_pct.clamp(0.0, 100.0);

        let index = find_nearest_bucket(x_pct);

        // Check if there's a paired bucket at this index
        let paired_index = if has_paired && index < bucketed2_signal.get_untracked().len() {
            Some(index)
        } else {
            None
        };

        hover.set(Some((index, paired_index)));
    };

    let on_mouse_leave = move |_ev: leptos::ev::MouseEvent| {
        hover.set(None);
    };

    // Render bars
    let bars = move || {
        if !has_data {
            return ().into_any();
        }

        let buckets = bucketed_signal.get();
        let buckets2 = bucketed2_signal.get();
        let c1 = color_signal.get();
        let c2 = color2_signal.get();
        let paired = !buckets2.is_empty();

        let slot_width = 100.0 / buckets.len() as f32;

        let rects: Vec<AnyView> = buckets
            .iter()
            .enumerate()
            .map(|(i, &val)| {
                let bar_height = (val / safe_max * height).max(1.0).clamp(1.0, height);
                let bar_y = (height - bar_height).clamp(0.0, height);
                let base_opacity = compute_opacity(val, safe_max);

                let (bar_width, bar_x) = if paired {
                    let half_slot = slot_width / 2.0;
                    let gap = 1.0_f32.max(slot_width * 0.05);
                    let w = (half_slot - gap).max(1.0);
                    (w, (i as f32 * slot_width) + gap)
                } else {
                    let gap = slot_width * 0.15;
                    let w = (slot_width - gap * 2.0).max(1.0);
                    (w, (i as f32 * slot_width) + gap)
                };

                let is_hovered = hover.get().map(|(idx, _)| idx == i).unwrap_or(false);
                let opacity = if is_hovered {
                    compute_hover_opacity(val, safe_max)
                } else {
                    base_opacity
                };

                let fill = c1.clone();
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
            .collect();

        // Paired bars (data2)
        let rects2: Vec<AnyView> = if paired {
            buckets2
                .iter()
                .enumerate()
                .map(|(i, &val)| {
                    let bar_height = (val / safe_max * height).max(1.0).clamp(1.0, height);
                    let bar_y = (height - bar_height).clamp(0.0, height);
                    let base_opacity = compute_opacity(val, safe_max);

                    let half_slot = slot_width / 2.0;
                    let gap = 1.0_f32.max(slot_width * 0.05);
                    let w = (half_slot - gap).max(1.0);
                    let x = (i as f32 * slot_width) + half_slot;

                    let is_hovered = hover
                        .get()
                        .map(|(_, pidx)| pidx == Some(i))
                        .unwrap_or(false);
                    let opacity = if is_hovered {
                        compute_hover_opacity(val, safe_max)
                    } else {
                        base_opacity
                    };

                    let fill = c2.clone();
                    view! {
                        <rect
                            x=x
                            y=bar_y
                            width=w
                            height=bar_height
                            fill=fill
                            fill-opacity=opacity
                            rx="2"
                            class="bar-rect"
                        />
                    }
                    .into_any()
                })
                .collect()
        } else {
            Vec::new()
        };

        view! {
            {rects}
            {rects2}
        }
        .into_any()
    };

    // Tooltip HTML element
    let tooltip_html = move || {
        hover.get().map(|(idx, _paired)| {
            let buckets = bucketed_signal.get();
            let buckets2 = bucketed2_signal.get();
            let unit = unit_label_signal.get();
            let c1 = color_signal.get();
            let c2 = color2_signal.get();

            if idx >= buckets.len() {
                return ().into_any();
            }

            let val = buckets[idx];
            let x_pct = bucket_x_pct(idx, buckets.len());
            let left_style = format!("left: {}%;", x_pct);

            // Get timestamp for this bucket
            let ts = if timestamps_valid && idx < timestamps.len() {
                Some(timestamps[idx])
            } else if timestamps_valid {
                // Map bucket index to approximate timestamp
                let oldest = *timestamps.first().unwrap_or(&0);
                let bucket_ms = 30_000i64;
                Some(oldest + (idx as i64) * bucket_ms)
            } else {
                None
            };

            let time_str = ts.map(format_relative_time).unwrap_or_default();

            let secondary = if !buckets2.is_empty() && idx < buckets2.len() {
                Some(buckets2[idx])
            } else {
                None
            };

            view! {
                <div class="sparkline-tooltip" style=left_style>
                    <span class="sparkline-tooltip-value" style=format!("color: {}", c1)>
                        {format!("{:.1}{}", val, if unit.is_empty() { "" } else { &unit })}
                    </span>
                    {if let Some(sec_val) = secondary {
                        view! {
                            <span class="sparkline-tooltip-value" style=format!("color: {}", c2)>
                                {format!("{:.1}{}", sec_val, if unit.is_empty() { "" } else { &unit })}
                            </span>
                        }.into_any()
                    } else {
                        ().into_any()
                    }}
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
    fn test_bucket_data_no_timestamps_returns_original() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        let result = bucket_data(&data, &[]);
        assert_eq!(result, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_bucket_data_length_mismatch_returns_original() {
        let data = vec![1.0, 2.0, 3.0, 4.0];
        let timestamps = vec![1000, 2000]; // wrong length
        let result = bucket_data(&data, &timestamps);
        assert_eq!(result, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn test_bucket_data_groups_into_30_second_windows() {
        // 4 data points within a 60-second span -> should bucket into 2 buckets
        let data = vec![10.0, 20.0, 30.0, 40.0];
        // First two in [0, 30s), last two in [30s, 60s)
        let timestamps = vec![0, 15_000, 30_000, 45_000];
        let result = bucket_data(&data, &timestamps);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 15.0).abs() < 0.01); // avg of 10 and 20
        assert!((result[1] - 35.0).abs() < 0.01); // avg of 30 and 40
    }

    #[test]
    fn test_bucket_data_single_point() {
        let data = vec![42.0];
        let timestamps = vec![1000];
        let result = bucket_data(&data, &timestamps);
        assert_eq!(result, vec![42.0]);
    }

    #[test]
    fn test_bucket_data_skips_empty_buckets() {
        // Points only in first and third 30s bucket, second is empty
        let data = vec![10.0, 30.0];
        let timestamps = vec![0, 60_000];
        let result = bucket_data(&data, &timestamps);
        assert_eq!(result.len(), 2);
        assert!((result[0] - 10.0).abs() < 0.01);
        assert!((result[1] - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_bucket_data_all_same_bucket() {
        // All points within 30 seconds
        let data = vec![1.0, 2.0, 3.0, 4.0, 5.0];
        let timestamps = vec![0, 5_000, 10_000, 20_000, 25_000];
        let result = bucket_data(&data, &timestamps);
        assert_eq!(result.len(), 1);
        assert!((result[0] - 3.0).abs() < 0.01); // avg of 1+2+3+4+5 = 15/5
    }

    #[test]
    fn test_empty_data() {
        let data: Vec<f32> = vec![];
        let timestamps: Vec<i64> = vec![];
        let result = bucket_data(&data, &timestamps);
        assert!(result.is_empty());
    }

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
