//! Kubernetes Container Runtime Interface (CRI v1) gRPC bindings for Coral.

#![allow(clippy::all)]

pub mod runtime {
    pub mod v1 {
        tonic::include_proto!("runtime.v1");
    }
}

pub use runtime::v1::*;
