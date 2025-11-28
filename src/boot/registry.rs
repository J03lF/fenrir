use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::services::{AppServices, ServiceRegistry, ServiceStatus};

pub(crate) fn register_registry_toggle_service(
    services: &AppServices,
    registry: &Arc<ServiceRegistry>,
    id: &'static str,
    active_status: ServiceStatus,
    active_note: &'static str,
    standby_status: ServiceStatus,
    standby_note: &'static str,
) {
    let state = Arc::new(AtomicBool::new(true));
    let registry_for_start = Arc::clone(registry);
    let registry_for_stop = Arc::clone(registry);
    services.register_dynamic_service(
        id,
        {
            let state = Arc::clone(&state);
            move || {
                let registry = Arc::clone(&registry_for_start);
                let state = Arc::clone(&state);
                Box::pin(async move {
                    let was_active = state.swap(true, Ordering::SeqCst);
                    if was_active {
                        Ok(false)
                    } else {
                        registry.set_status(id, active_status, Some(active_note.to_string()));
                        Ok(true)
                    }
                })
            }
        },
        {
            let state = Arc::clone(&state);
            move |_force| {
                let registry = Arc::clone(&registry_for_stop);
                let state = Arc::clone(&state);
                Box::pin(async move {
                    let was_active = state.swap(false, Ordering::SeqCst);
                    if !was_active {
                        Ok(false)
                    } else {
                        registry.set_status(id, standby_status, Some(standby_note.to_string()));
                        Ok(true)
                    }
                })
            }
        },
    );
}
