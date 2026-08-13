pub mod client;
pub mod protocol;

pub mod tamad_service {
    include!(concat!(env!("OUT_DIR"), "/tamad.rs"));
}

pub use tamad_service::tamad_service_client::TamadServiceClient;
pub use tamad_service::tamad_service_server::{TamadService, TamadServiceServer};

// Re-export generated message types for convenience
pub use tamad_service::{
    Empty, HealthResponse, InstallProviderRequest, InstallProviderResponse, ListProvidersResponse,
    LoadModelRequest, LoadModelResponse, LogEntry, LogsRequest, ProviderInfo,
    RemoveProviderRequest, UnloadModelRequest, UpdateProviderRequest, UpdateProviderResponse,
};
