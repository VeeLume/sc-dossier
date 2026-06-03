//! Generated prost/tonic types (internal). Module tree mirrors protobuf package
//! paths so cross-package references resolve.

#![allow(clippy::all, dead_code, missing_docs, rustdoc::all)]

pub(crate) mod sc {
    pub(crate) mod external {
        pub(crate) mod services {
            pub(crate) mod blueprint_library {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.services.blueprint_library.v1");
                }
            }
            pub(crate) mod entitlement {
                pub(crate) mod v2 {
                    tonic::include_proto!("sc.external.services.entitlement.v2");
                }
            }
            pub(crate) mod identity {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.services.identity.v1");
                }
            }
            pub(crate) mod entitygraph {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.services.entitygraph.v1");
                }
            }
        }
        pub(crate) mod common {
            pub(crate) mod api {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.common.api.v1");
                }
            }
            pub(crate) mod types {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.common.types.v1");
                }
            }
            pub(crate) mod game {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.common.game.v1");
                }
            }
        }
    }
}

pub(crate) use sc::external::common::api::v1 as common_api;
pub(crate) use sc::external::services::blueprint_library::v1 as blueprint_library;
pub(crate) use sc::external::services::entitlement::v2 as entitlement;
pub(crate) use sc::external::services::entitygraph::v1 as entitygraph;
pub(crate) use sc::external::services::identity::v1 as identity;
