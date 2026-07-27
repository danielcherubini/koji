use leptos::prelude::*;

use crate::components::section_card::SectionCard;
use crate::core_mirrors::RestartPolicy as CoreRestartPolicy;
use crate::utils::target_value;

#[component]
pub fn LifecycleForm(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_lc = move || config.get().map(|c| c.lifecycle).unwrap_or_default();

    view! {
        <SectionCard title="Lifecycle".to_string() description=Some("Process restart and health-check behavior for managed models.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Restart Policy"</label>
                    <select
                        prop:value=move || get_lc().restart_policy.as_str().to_string()
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| {
                                if let Some(c) = c {
                                    c.lifecycle.restart_policy = CoreRestartPolicy::from_str(&v);
                                }
                            });
                        }
                    >
                        <option value="always">"always"</option>
                        <option value="on-failure">"on-failure"</option>
                    </select>
                </div>

                <div>
                    <label>"Max Restarts"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_lc().max_restarts.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.lifecycle.max_restarts = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Restart Delay (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_lc().restart_delay_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.lifecycle.restart_delay_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Interval (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_lc().health_check_interval_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.lifecycle.health_check_interval_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Timeout (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_lc().health_check_timeout_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.lifecycle.health_check_timeout_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Retries"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_lc().health_check_retries.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.lifecycle.health_check_retries = v; });
                            }
                        }
                    />
                </div>
            </div>
        </SectionCard>
    }
}
