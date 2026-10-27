//! Engine version reporting.

/// App version stamped by the build script or default "dev".
pub fn app_version() -> &'static str {
    option_env!("APP_VERSION").unwrap_or("dev")
}
