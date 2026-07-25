/// Format a statistic as "mean ± stddev" or just "mean" if stddev is 0.0
pub fn format_stat(mean: f64, stddev: f64) -> String {
    if stddev == 0.0 {
        format!("{:.1}", mean)
    } else {
        format!("{:.1} ± {:.1}", mean, stddev)
    }
}
