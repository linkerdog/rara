//! Session ownership and host-controlled execution over the shared agent loop.

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
compile_error!(
    "rara-runtime currently uses the native Tokio session executor; browser hosts can use rara-core and rara-agent until the browser session adapter is available"
);

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
mod native;
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub use native::*;
