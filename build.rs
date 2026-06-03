//! Compiles the `.proto` files into prost/tonic client code. `protoc` is
//! supplied by `protoc-bin-vendored`; generated code is included from
//! `src/proto.rs`.

use std::error::Error;

const PROTO_FILES: &[&str] = &[
    "proto/sc/external/services/blueprint_library/v1/types.proto",
    "proto/sc/external/services/blueprint_library/v1/api.proto",
    "proto/sc/external/services/entitlement/v2/types.proto",
    "proto/sc/external/services/entitlement/v2/api.proto",
    "proto/sc/external/services/identity/v1/player.proto",
    "proto/sc/external/services/identity/v1/api.proto",
    "proto/sc/external/services/entitygraph/v1/types.proto",
    "proto/sc/external/services/entitygraph/v1/query.proto",
    "proto/sc/external/services/entitygraph/v1/api.proto",
    "proto/sc/external/common/types/v1/localization.proto",
    "proto/sc/external/common/types/v1/transforms.proto",
    "proto/sc/external/common/game/v1/types.proto",
    "proto/sc/external/common/api/v1/pagination.proto",
    "proto/sc/external/common/api/v1/query.proto",
];

const INCLUDE_DIRS: &[&str] = &["proto"];

fn main() -> Result<(), Box<dyn Error>> {
    let protoc = protoc_bin_vendored::protoc_bin_path()?;
    // SAFETY: build scripts are single-threaded.
    std::env::set_var("PROTOC", protoc);

    for f in PROTO_FILES {
        println!("cargo:rerun-if-changed={f}");
    }

    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(PROTO_FILES, INCLUDE_DIRS)?;

    Ok(())
}
