//! Kubernetes CRI v1 `RuntimeService` gRPC implementation for `CoralDaemonService`.
//!
//! The trait implementation itself stays in this file (Rust allows only one
//! `impl RuntimeService for CoralDaemonService` per crate); every RPC body
//! lives in a sibling module as a free function that takes the service by
//! reference.

mod cni;
mod create;
mod exec;
mod lifecycle;
mod pods;
mod stats;

use super::pty_rpc::ExecStreamOutput;
use super::CoralDaemonService;
use coral_api::runtime_service_server::RuntimeService;
use coral_api::{
    AttachRequest, AttachResponse, ContainerStatsRequest, ContainerStatsResponse,
    ContainerStatusRequest, ContainerStatusResponse, CreateContainerRequest,
    CreateContainerResponse, ExecRequest, ExecResponse, ExecStreamRequest, ExecSyncRequest,
    ExecSyncResponse, ListContainerStatsRequest, ListContainerStatsResponse,
    ListContainersRequest, ListContainersResponse, ListPodSandboxRequest,
    ListPodSandboxResponse, PodSandboxStatusRequest, PodSandboxStatusResponse,
    PortForwardRequest, PortForwardResponse, RemoveContainerRequest, RemoveContainerResponse,
    RemovePodSandboxRequest, RemovePodSandboxResponse, ReopenContainerLogRequest,
    ReopenContainerLogResponse, RunPodSandboxRequest, RunPodSandboxResponse,
    StartContainerRequest, StartContainerResponse, StatusRequest, StatusResponse,
    StopContainerRequest, StopContainerResponse, StopPodSandboxRequest,
    StopPodSandboxResponse, UpdateRuntimeConfigRequest, UpdateRuntimeConfigResponse,
    VersionRequest, VersionResponse,
};
use tonic::{Request, Response, Status, Streaming};

pub(crate) use cni::build_cni_exec_request;

#[tonic::async_trait]
impl RuntimeService for CoralDaemonService {
    type ExecStreamStream = ExecStreamOutput;
    async fn version(
        &self,
        _request: Request<VersionRequest>,
    ) -> Result<Response<VersionResponse>, Status> {
        self::pods::version(self, _request).await
    }

    async fn status(
        &self,
        request: Request<StatusRequest>,
    ) -> Result<Response<StatusResponse>, Status> {
        self::pods::status(self, request).await
    }

    async fn pod_sandbox_status(
        &self,
        request: Request<PodSandboxStatusRequest>,
    ) -> Result<Response<PodSandboxStatusResponse>, Status> {
        self::pods::pod_sandbox_status(self, request).await
    }

    async fn run_pod_sandbox(
        &self,
        request: Request<RunPodSandboxRequest>,
    ) -> Result<Response<RunPodSandboxResponse>, Status> {
        self::pods::run_pod_sandbox(self, request).await
    }

    async fn stop_pod_sandbox(
        &self,
        request: Request<StopPodSandboxRequest>,
    ) -> Result<Response<StopPodSandboxResponse>, Status> {
        self::pods::stop_pod_sandbox(self, request).await
    }

    async fn remove_pod_sandbox(
        &self,
        request: Request<RemovePodSandboxRequest>,
    ) -> Result<Response<RemovePodSandboxResponse>, Status> {
        self::pods::remove_pod_sandbox(self, request).await
    }

    async fn list_pod_sandbox(
        &self,
        request: Request<ListPodSandboxRequest>,
    ) -> Result<Response<ListPodSandboxResponse>, Status> {
        self::pods::list_pod_sandbox(self, request).await
    }

    async fn create_container(
        &self,
        request: Request<CreateContainerRequest>,
    ) -> Result<Response<CreateContainerResponse>, Status> {
        self::create::create_container(self, request).await
    }

    async fn container_status(
        &self,
        request: Request<ContainerStatusRequest>,
    ) -> Result<Response<ContainerStatusResponse>, Status> {
        self::create::container_status(self, request).await
    }

    async fn start_container(
        &self,
        request: Request<StartContainerRequest>,
    ) -> Result<Response<StartContainerResponse>, Status> {
        self::lifecycle::start_container(self, request).await
    }

    async fn stop_container(
        &self,
        request: Request<StopContainerRequest>,
    ) -> Result<Response<StopContainerResponse>, Status> {
        self::lifecycle::stop_container(self, request).await
    }

    async fn remove_container(
        &self,
        request: Request<RemoveContainerRequest>,
    ) -> Result<Response<RemoveContainerResponse>, Status> {
        self::lifecycle::remove_container(self, request).await
    }

    async fn list_containers(
        &self,
        request: Request<ListContainersRequest>,
    ) -> Result<Response<ListContainersResponse>, Status> {
        self::lifecycle::list_containers(self, request).await
    }

    async fn exec_sync(
        &self,
        request: Request<ExecSyncRequest>,
    ) -> Result<Response<ExecSyncResponse>, Status> {
        self::exec::exec_sync(self, request).await
    }

    async fn exec(
        &self,
        request: Request<ExecRequest>,
    ) -> Result<Response<ExecResponse>, Status> {
        self::exec::exec(self, request).await
    }

    async fn attach(
        &self,
        request: Request<AttachRequest>,
    ) -> Result<Response<AttachResponse>, Status> {
        self::exec::attach(self, request).await
    }

    async fn port_forward(
        &self,
        request: Request<PortForwardRequest>,
    ) -> Result<Response<PortForwardResponse>, Status> {
        self::exec::port_forward(self, request).await
    }

    async fn exec_stream(
        &self,
        request: Request<Streaming<ExecStreamRequest>>,
    ) -> Result<Response<Self::ExecStreamStream>, Status> {
        self::exec::exec_stream(self, request).await
    }

    async fn container_stats(
        &self,
        request: Request<ContainerStatsRequest>,
    ) -> Result<Response<ContainerStatsResponse>, Status> {
        self::stats::container_stats(self, request).await
    }

    async fn list_container_stats(
        &self,
        request: Request<ListContainerStatsRequest>,
    ) -> Result<Response<ListContainerStatsResponse>, Status> {
        self::stats::list_container_stats(self, request).await
    }

    async fn update_runtime_config(
        &self,
        request: Request<UpdateRuntimeConfigRequest>,
    ) -> Result<Response<UpdateRuntimeConfigResponse>, Status> {
        self::stats::update_runtime_config(self, request).await
    }

    async fn reopen_container_log(
        &self,
        request: Request<ReopenContainerLogRequest>,
    ) -> Result<Response<ReopenContainerLogResponse>, Status> {
        self::stats::reopen_container_log(self, request).await
    }
}
