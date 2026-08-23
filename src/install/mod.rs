//! Runtime installation and the fallback chain.
//!
//! Importance order (highest first): nodejs → bun → fnm → cargo → nvm.
//!   * nodejs is **required** (dsh runs on Node); min 24.15.0, we install 26.
//!   * bun is installed only when the config's `pm` asks for it.
//!   * node 26 is installed via fnm first, then `cargo install fnm`, then nvm,
//!     then a best-effort fnm auto-install into `~/.cache/bin`; if everything
//!     fails the UI is told to install fnm manually.
//!
//! The module is split into small cohesive pieces (loose coupling):
//!
//! - [`runtime`]: the resolved runtime model ([`Runtime`] — which directories
//!   to prepend to `PATH`).
//! - [`stream`]: [`run_streaming`] — stream a command's output into the
//!   progress log and fail on non-zero exit.
//! - [`node`]: [`ensure_node`] and the fnm → cargo → nvm → auto-install
//!   fallback chain.
//! - [`bun`]: [`ensure_bun`] and the direct-download / official-script / npm
//!   fallback chain.
//! - [`pnpm`]: [`ensure_pnpm`] and the global-bin-dir resolution.
//! - [`nub`]: [`ensure_nub`] — the @nubjs/nub toolkit installed via npm into
//!   the cache (mirrorable); replaces the package manager and, in a future
//!   tier, Node provisioning (`nub node install|which`).
//! - [`download`]: zip download + extraction and small file helpers shared by
//!   the installers.

pub mod bun;
pub mod download;
pub mod node;
pub mod nub;
pub mod pnpm;
pub mod runtime;
pub mod stream;

pub use bun::ensure_bun;
pub use node::ensure_node;
pub use nub::ensure_nub;
pub use pnpm::ensure_pnpm;
pub use runtime::Runtime;
pub use stream::run_streaming;

/// Minimum Node.js version required by dsh.
pub const NODE_MIN: crate::version::Version = crate::version::Version::new(24, 15, 0);
/// Minimum bun version.
pub const BUN_MIN: crate::version::Version = crate::version::Version::new(1, 3, 14);
/// Node.js major version to install when missing.
pub const NODE_INSTALL_VERSION: &str = "26";
/// fnm manual-install guide shown when every fallback fails.
pub const FNM_GUIDE_URL: &str = "https://www.fnmnode.com/zh-cn/guide/install";
