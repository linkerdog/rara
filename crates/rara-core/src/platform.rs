/// Requires thread transfer on native targets; browser effects stay local.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub trait PlatformSend: Send {}
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl<T: Send + ?Sized> PlatformSend for T {}

/// Allows browser-owned values to remain on their JavaScript executor.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub trait PlatformSend {}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl<T: ?Sized> PlatformSend for T {}

/// Requires shared thread safety on native targets; browser effects stay local.
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub trait PlatformSync: Sync {}
#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl<T: Sync + ?Sized> PlatformSync for T {}

/// Allows browser-owned references without implying cross-thread safety.
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub trait PlatformSync {}
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl<T: ?Sized> PlatformSync for T {}
