use leptos::prelude::*;

use crate::components::section_card::SectionCard;
use crate::gpu_types::RestartPolicy as CoreRestartPolicy;
use crate::utils::target_value;

#[component]
pub fn SupervisorForm(
    config: RwSignal<Option<crate::pages::config_editor::types::Config>>,
) -> impl IntoView {
    let get_sup = move || config.get().map(|c| c.supervisor).unwrap_or_default();

    view! {
        <SectionCard title="Supervisor".to_string() description=Some("Process restart and health-check behavior for managed models.".to_string())>

            <div style="display:flex;flex-direction:column;gap:1rem;margin-top:1rem;">
                <div>
                    <label>"Restart Policy"</label>
                    <select
                        prop:value=move || get_sup().restart_policy.as_str().to_string()
                        on:change=move |ev| {
                            let v = target_value(&ev);
                            config.update(|c| {
                                if let Some(c) = c {
                                    c.supervisor.restart_policy = CoreRestartPolicy::from_str(&v);
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
                        prop:value=move || get_sup().max_restarts.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.max_restarts = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Restart Delay (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().restart_delay_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.restart_delay_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Interval (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_interval_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_interval_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Timeout (ms)"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_timeout_ms.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u64>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_timeout_ms = v; });
                            }
                        }
                    />
                </div>

                <div>
                    <label>"Health Check Retries"</label>
                    <input
                        type="number"
                        min="0"
                        prop:value=move || get_sup().health_check_retries.to_string()
                        on:input=move |ev| {
                            if let Ok(v) = target_value(&ev).parse::<u32>() {
                                config.update(|c| if let Some(c) = c { c.supervisor.health_check_retries = v; });
                            }
                        }
                    />
                </div>
            </div>
        </SectionCard>
    }
}
