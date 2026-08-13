use std::net::SocketAddr;

use anyhow::{anyhow, Result};
use async_trait::async_trait;
use axum::Router;
use tonic::transport::Server as TonicServer;
use tracing::info;

use tama_core::tamad::tamad_service::Empty as GrpcEmpty;
use tama_core::tamad::tamad_service::HealthResponse;
use tama_core::tamad::tamad_service::InstallProviderRequest;
use tama_core::tamad::tamad_service::InstallProviderResponse;
use tama_core::tamad::tamad_service::ListProvidersResponse;
use tama_core::tamad::tamad_service::LoadModelRequest as GrpcLoadModelRequest;
use tama_core::tamad::tamad_service::LoadModelResponse as GrpcLoadModelResponse;
use tama_core::tamad::tamad_service::LogEntry;
use tama_core::tamad::tamad_service::LogsRequest;
use tama_core::tamad::tamad_service::RemoveProviderRequest;
use tama_core::tamad::tamad_service::UnloadModelRequest;
use tama_core::tamad::tamad_service::UpdateProviderRequest;
use tama_core::tamad::tamad_service::UpdateProviderResponse;
use tama_core::tamad::TamadService;

const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, Default)]
pub struct TamadServiceImpl;

#[async_trait]
impl TamadService for TamadServiceImpl {
    async fn list_providers(
        &self,
        _request: tonic::Request<GrpcEmpty>,
    ) -> std::result::Result<tonic::Response<ListProvidersResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn install_provider(
        &self,
        _request: tonic::Request<InstallProviderRequest>,
    ) -> std::result::Result<tonic::Response<InstallProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn load_model(
        &self,
        _request: tonic::Request<GrpcLoadModelRequest>,
    ) -> std::result::Result<tonic::Response<GrpcLoadModelResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn unload_model(
        &self,
        _request: tonic::Request<UnloadModelRequest>,
    ) -> std::result::Result<tonic::Response<GrpcEmpty>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn update_provider(
        &self,
        _request: tonic::Request<UpdateProviderRequest>,
    ) -> std::result::Result<tonic::Response<UpdateProviderResponse>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn remove_provider(
        &self,
        _request: tonic::Request<RemoveProviderRequest>,
    ) -> std::result::Result<tonic::Response<GrpcEmpty>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    type LogsStream = tokio_stream::Iter<std::vec::IntoIter<Result<LogEntry, tonic::Status>>>;

    async fn logs(
        &self,
        _request: tonic::Request<LogsRequest>,
    ) -> std::result::Result<tonic::Response<Self::LogsStream>, tonic::Status> {
        Err(tonic::Status::unimplemented("not implemented"))
    }

    async fn health_check(
        &self,
        _request: tonic::Request<GrpcEmpty>,
    ) -> std::result::Result<tonic::Response<HealthResponse>, tonic::Status> {
        Ok(tonic::Response::new(HealthResponse {
            status: "ok".to_string(),
            version: VERSION.to_string(),
        }))
    }
}

pub async fn health_handler() -> String {
    serde_json::json!({ "status": "ok", "version": VERSION }).to_string()
}

pub async fn start(addr: &str, protocol: &str) -> Result<()> {
    let addr: SocketAddr = addr
        .parse()
        .map_err(|e| anyhow!("Invalid address '{}': {}", addr, e))?;

    let service = TamadServiceImpl;

    let grpc_task = match protocol {
        "grpc" | "both" => {
            let grpc_addr = addr;
            Some(tokio::spawn(async move {
                info!(%grpc_addr, "Starting gRPC server");
                let serve = TonicServer::builder()
                    .add_service(tama_core::tamad::TamadServiceServer::new(service))
                    .serve(grpc_addr);

                if let Err(e) = serve.await {
                    tracing::error!(error = %e, "gRPC server error");
                }
            }))
        }
        _ => None,
    };

    let http_task = match protocol {
        "http" | "both" => {
            let http_addr: SocketAddr = if protocol == "both" {
                let mut a = addr;
                a.set_port(addr.port() + 1);
                a
            } else {
                addr
            };

            info!(%http_addr, "Starting HTTP server");

            let app = Router::new().route("/health", axum::routing::get(health_handler));

            Some(tokio::spawn(async move {
                match axum::serve(tokio::net::TcpListener::bind(http_addr).await.unwrap(), app)
                    .await
                {
                    Ok(()) => {}
                    Err(e) => tracing::error!(error = %e, "HTTP server error"),
                }
            }))
        }
        _ => None,
    };

    // Wait for all running tasks
    if let Some(task) = grpc_task {
        let _ = task.await;
    }
    if let Some(task) = http_task {
        let _ = task.await;
    }

    Ok(())
}
