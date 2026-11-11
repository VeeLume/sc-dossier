//! Generated prost/tonic types (internal). Module tree mirrors protobuf package
//! paths so cross-package references resolve.

#![allow(clippy::all, dead_code, missing_docs, rustdoc::all)]

pub(crate) mod sc {
    #[cfg(feature = "backend")]
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
            pub(crate) mod mission_service {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.services.mission_service.v1");
                }
            }
            pub(crate) mod reputation {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.services.reputation.v1");
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
            pub(crate) mod mission {
                pub(crate) mod v1 {
                    tonic::include_proto!("sc.external.common.mission.v1");
                }
            }
        }
    }
    #[cfg(feature = "telemetry")]
    pub(crate) mod game {
        pub(crate) mod telemetry {
            pub(crate) mod v1 {
                tonic::include_proto!("sc.game.telemetry.v1");
            }
        }
    }
}

#[cfg(feature = "telemetry")]
pub(crate) use sc::game::telemetry::v1 as telemetry;

#[cfg(feature = "backend")]
pub(crate) use sc::external::common::api::v1 as common_api;
#[cfg(feature = "backend")]
pub(crate) use sc::external::common::mission::v1 as mission;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::blueprint_library::v1 as blueprint_library;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::entitlement::v2 as entitlement;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::entitygraph::v1 as entitygraph;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::identity::v1 as identity;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::mission_service::v1 as mission_service;
#[cfg(feature = "backend")]
pub(crate) use sc::external::services::reputation::v1 as reputation;
