//! The scope is derived from `CommunityConfig`; this module intentionally has
//! no compile-time community identifiers.

/// Reviewed transitive scope edges for legacy views that inherit the boundary
/// from another `mcp_public` view. Any new edge requires an explicit review.
pub const INHERITED_SCOPE_VIEW_DEPENDENCIES: &[(&str, &str)] =
    &[("avatar_image_analyses", "telegram_user_profiles")];

pub fn inherited_scope_parent(view: &str) -> Option<&'static str> {
    INHERITED_SCOPE_VIEW_DEPENDENCIES
        .iter()
        .find_map(|(child, parent)| (*child == view).then_some(*parent))
}
