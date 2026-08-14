use crate::components::pull_wizard::*;
use std::collections::HashSet;

#[component]
pub fn SelectionStep(
    repo_id: Signal<String>,
    available_quants: Signal<Vec<QuantEntry>>,
    available_mmprojs: Signal<Vec<QuantEntry>>,
    available_mtps: Signal<Vec<QuantEntry>>,
    selected_filenames: RwSignal<HashSet<String>>,
    selected_mmproj_filenames: RwSignal<HashSet<String>>,
    selected_mtp_filenames: RwSignal<HashSet<String>>,
    on_next: Callback<()>,
    on_back: Callback<()>,
) -> impl IntoView {
    view! {
        <div class="form-card__header">
            <h2 class="form-card__title">"Select Quants"</h2>
            <p class="form-card__desc text-muted">
                "Choose one or more quant files to pull from "
                <code>{move || repo_id.get()}</code>"."
            </p>
        </div>

        <div class="form-actions mb-2">
            <button class="btn btn-secondary btn-sm" on:click=move |_| {
                selected_filenames.set(collect_all_filenames(&available_quants.get()));
            }>
                "Select All"
            </button>
            <button class="btn btn-secondary btn-sm" on:click=move |_| {
                selected_filenames.set(HashSet::new());
            }>
                "Deselect All"
            </button>
        </div>

        <table class="data-table">
            <thead>
                <tr>
                    <th class="icon-sm"></th>
                    <th>"Quant"</th>
                    <th>"Filename"</th>
                    <th>"Size"</th>
                </tr>
            </thead>
            <tbody>
                {move || available_quants.get().into_iter().map(|q| {
                    let fname = q.filename.clone();
                    let fname_check = fname.clone();
                    let shards = q.shards.clone();
                    let label = q.quant.clone().unwrap_or_else(|| fname.clone());
                    let size_str = q.size_bytes
                        .map(|b| format_bytes(b as u64))
                        .unwrap_or_else(|| "?".to_string());
                    let is_checked = move || selected_filenames.get().contains(&fname_check);
                    view! {
                        <tr>
                            <td>
                                <input
                                    type="checkbox"
                                    prop:checked=is_checked
                                    on:change=move |_| {
                                        let shards = shards.clone();
                                        selected_filenames.update(|set| {
                                            toggle_quant_selection(set, &fname, &shards);
                                        });
                                    }
                                />
                            </td>
                            <td>
                                <span class="badge badge-info">{label}</span>
                            </td>
                            <td><code>{q.filename.clone()}</code></td>
                            <td class="text-muted">{size_str}</td>
                        </tr>
                    }
                }).collect::<Vec<_>>()}
            </tbody>
        </table>

        <div class="mt-4 mb-2">
            <h3 class="form-label">"Vision Projectors"</h3>
            <p class="text-muted text-sm mb-2">"Select vision projectors (mmproj) for this model."</p>
            <table class="data-table">
                <thead>
                    <tr>
                        <th class="icon-sm"></th>
                        <th>"Filename"</th>
                        <th>"Size"</th>
                    </tr>
                </thead>
                <tbody>
                    {move || available_mmprojs.get().into_iter().map(|q| {
                        let fname = q.filename.clone();
                        let fname_check = fname.clone();
                        let size_str = q.size_bytes
                            .map(|b| format_bytes(b as u64))
                            .unwrap_or_else(|| "?".to_string());
                        let is_checked = move || selected_mmproj_filenames.get().contains(&fname_check);
                        view! {
                            <tr>
                                <td>
                                    <input
                                        type="checkbox"
                                        prop:checked=is_checked
                                        on:change=move |_| {
                                            selected_mmproj_filenames.update(|set| {
                                                if set.contains(&fname) {
                                                    set.remove(&fname);
                                                } else {
                                                    set.insert(fname.clone());
                                                }
                                            });
                                        }
                                    />
                                </td>
                                <td><code>{q.filename.clone()}</code></td>
                                <td class="text-muted">{size_str}</td>
                            </tr>
                        }
                    }).collect::<Vec<_>>()}
                </tbody>
            </table>
        </div>

        <Show when=move || !available_mtps.get().is_empty()>
            <div class="mt-4 mb-2">
                <h3 class="form-label">"MTP Draft Models"</h3>
                <p class="text-muted text-sm mb-2">"Select MTP draft model files for speculative decoding (mtp-*.gguf)."</p>
                <table class="data-table">
                    <thead>
                        <tr>
                            <th class="icon-sm"></th>
                            <th>"Filename"</th>
                            <th>"Size"</th>
                        </tr>
                    </thead>
                    <tbody>
                        {move || available_mtps.get().into_iter().map(|q| {
                            let fname = q.filename.clone();
                            let fname_check = fname.clone();
                            let size_str = q.size_bytes
                                .map(|b| format_bytes(b as u64))
                                .unwrap_or_else(|| "?".to_string());
                            let is_checked = move || selected_mtp_filenames.get().contains(&fname_check);
                            view! {
                                <tr>
                                    <td>
                                        <input
                                            type="checkbox"
                                            prop:checked=is_checked
                                            on:change=move |_| {
                                                selected_mtp_filenames.update(|set| {
                                                    if set.contains(&fname) {
                                                        set.remove(&fname);
                                                    } else {
                                                        set.insert(fname.clone());
                                                    }
                                                });
                                            }
                                        />
                                    </td>
                                    <td><code>{q.filename.clone()}</code></td>
                                    <td class="text-muted">{size_str}</td>
                                </tr>
                            }
                        }).collect::<Vec<_>>()}
                    </tbody>
                </table>
            </div>
        </Show>

        <div class="form-actions mt-3">
            <Show when=move || !repo_id.get().trim().is_empty()>
                <button class="btn btn-secondary" on:click=move |_| on_back.run(())>
                    "Back"
                </button>
            </Show>
            <button
                class="btn btn-primary"
                prop:disabled=move || selected_filenames.get().is_empty() && selected_mmproj_filenames.get().is_empty() && selected_mtp_filenames.get().is_empty()
                on:click=move |_| on_next.run(())
            >
                "Next →"
            </button>
        </div>
    }
}

// ── Pure helper functions (extracted for testability) ────────────────────────

/// Collect all filenames (primary + shards) from a list of quant entries.
/// Used by the "Select All" button to ensure shard filenames are included.
fn collect_all_filenames(quants: &[QuantEntry]) -> HashSet<String> {
    let mut all = HashSet::new();
    for q in quants {
        all.insert(q.filename.clone());
        for s in &q.shards {
            all.insert(s.clone());
        }
    }
    all
}

/// Toggle a quant's selection in the set: when checking, insert the primary
/// filename AND all shard filenames; when unchecking, remove them all.
/// The toggle direction is determined by whether the primary filename is
/// already in the set.
fn toggle_quant_selection(set: &mut HashSet<String>, filename: &str, shards: &[String]) {
    if set.contains(filename) {
        set.remove(filename);
        for s in shards {
            set.remove(s);
        }
    } else {
        set.insert(filename.to_string());
        for s in shards {
            set.insert(s.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_quant(filename: &str, shards: Vec<String>) -> QuantEntry {
        QuantEntry {
            filename: filename.to_string(),
            quant: Some(filename.to_string()),
            size_bytes: None,
            kind: QuantKind::Model,
            shards,
        }
    }

    #[test]
    fn test_collect_all_filenames_single_file() {
        let quants = vec![
            make_quant("model-Q4_K_M.gguf", vec![]),
            make_quant("model-Q8_0.gguf", vec![]),
        ];
        let result = collect_all_filenames(&quants);
        assert!(result.contains("model-Q4_K_M.gguf"));
        assert!(result.contains("model-Q8_0.gguf"));
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn test_collect_all_filenames_with_shards() {
        let quants = vec![
            make_quant(
                "UD-Q4_K_XL/UD-Q4_K_XL-00001-of-00003.gguf",
                vec![
                    "UD-Q4_K_XL/UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
                    "UD-Q4_K_XL/UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
                    "UD-Q4_K_XL/UD-Q4_K_XL-00003-of-00003.gguf".to_string(),
                ],
            ),
            make_quant("model-Q4_K_M.gguf", vec![]),
        ];
        let result = collect_all_filenames(&quants);
        // Primary filename is the first shard's full path (matches real API shape)
        assert!(result.contains("UD-Q4_K_XL/UD-Q4_K_XL-00001-of-00003.gguf"));
        assert!(result.contains("model-Q4_K_M.gguf"));
        // Remaining shard filenames (shard 1 == primary above)
        assert!(result.contains("UD-Q4_K_XL/UD-Q4_K_XL-00002-of-00003.gguf"));
        assert!(result.contains("UD-Q4_K_XL/UD-Q4_K_XL-00003-of-00003.gguf"));
        // 2 primaries + 2 unique shards (shard 1 == primary) = 4 total
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_toggle_quant_selection_check_single() {
        let mut set = HashSet::new();
        toggle_quant_selection(&mut set, "model-Q4_K_M.gguf", &[]);
        assert!(set.contains("model-Q4_K_M.gguf"));
    }

    #[test]
    fn test_toggle_quant_selection_uncheck_single() {
        let mut set: HashSet<String> = ["model-Q4_K_M.gguf".to_string()].into_iter().collect();
        toggle_quant_selection(&mut set, "model-Q4_K_M.gguf", &[]);
        assert!(!set.contains("model-Q4_K_M.gguf"));
        assert!(set.is_empty());
    }

    #[test]
    fn test_toggle_quant_selection_check_with_shards() {
        let shards = vec![
            "UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
            "UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
        ];
        let mut set = HashSet::new();
        // Check: insert primary + all shards
        toggle_quant_selection(&mut set, "UD-Q4_K_XL.gguf", &shards);
        assert!(set.contains("UD-Q4_K_XL.gguf"));
        assert!(set.contains("UD-Q4_K_XL-00001-of-00003.gguf"));
        assert!(set.contains("UD-Q4_K_XL-00002-of-00003.gguf"));
    }

    #[test]
    fn test_toggle_quant_selection_uncheck_with_shards() {
        let shards = vec![
            "UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
            "UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
        ];
        // Start with primary + shards already selected
        let mut set: HashSet<String> = [
            "UD-Q4_K_XL.gguf".to_string(),
            "UD-Q4_K_XL-00001-of-00003.gguf".to_string(),
            "UD-Q4_K_XL-00002-of-00003.gguf".to_string(),
        ]
        .into_iter()
        .collect();
        // Uncheck: remove primary + all shards
        toggle_quant_selection(&mut set, "UD-Q4_K_XL.gguf", &shards);
        assert!(!set.contains("UD-Q4_K_XL.gguf"));
        assert!(!set.contains("UD-Q4_K_XL-00001-of-00003.gguf"));
        assert!(!set.contains("UD-Q4_K_XL-00002-of-00003.gguf"));
        assert!(set.is_empty());
    }
}
