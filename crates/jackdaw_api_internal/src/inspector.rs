//! Registry of inspector category tabs and the component-type routing table.

use std::borrow::Cow;
use std::collections::HashMap;

use bevy::prelude::Resource;
use lucide_icons::Icon;

/// One inspector category tab. `id` is a stable string key so the set is open
/// to extensions; built-ins use the ids "object", "mesh", "modifiers",
/// "material", "physics", "components".
#[derive(Clone, Debug)]
pub struct InspectorCategory {
    pub id: Cow<'static, str>,
    pub label: Cow<'static, str>,
    pub icon: Icon,
    pub order: i32,
}

/// Registry of inspector categories and the component-type-to-category routing
/// table. Seeded with six defaults; extensions add entries via
/// `ExtensionContext`.
#[derive(Resource, Default)]
pub struct InspectorRegistry {
    categories: Vec<InspectorCategory>,
    component_category: HashMap<Cow<'static, str>, Cow<'static, str>>,
    component_category_prefixes: Vec<(Cow<'static, str>, Cow<'static, str>)>,
}

impl InspectorRegistry {
    /// Add or replace a category by id. The most recently written entry for a
    /// given id wins; all other fields (label, icon, order) are replaced too.
    pub fn register_category(&mut self, category: InspectorCategory) {
        if let Some(slot) = self.categories.iter_mut().find(|c| c.id == category.id) {
            *slot = category;
        } else {
            self.categories.push(category);
        }
    }

    /// Route a component reflect type path to a category id.
    pub fn set_component_category(
        &mut self,
        type_path: impl Into<Cow<'static, str>>,
        category_id: impl Into<Cow<'static, str>>,
    ) {
        self.component_category
            .insert(type_path.into(), category_id.into());
    }

    /// Route all type paths that start with `prefix` to a category id.
    /// Exact-path mappings take precedence over prefix matches.
    pub fn set_component_category_prefix(
        &mut self,
        prefix: impl Into<Cow<'static, str>>,
        category_id: impl Into<Cow<'static, str>>,
    ) {
        self.component_category_prefixes
            .push((prefix.into(), category_id.into()));
    }

    /// The category id a component type path resolves to.
    ///
    /// Resolution order:
    /// 1. Exact match in the per-path map.
    /// 2. First prefix match (in insertion order).
    /// 3. Falls back to "components" when nothing matches or the matched
    ///    category id is not registered.
    pub fn category_for(&self, type_path: &str) -> &str {
        if let Some(id) = self.component_category.get(type_path)
            && self.categories.iter().any(|c| c.id == *id)
        {
            return id.as_ref();
        }
        for (prefix, cat_id) in &self.component_category_prefixes {
            if type_path.starts_with(prefix.as_ref())
                && self.categories.iter().any(|c| c.id == *cat_id)
            {
                return cat_id.as_ref();
            }
        }
        "components"
    }

    /// Categories in strip order, sorted by `order` field, preserving
    /// insertion order within ties.
    pub fn categories_sorted(&self) -> Vec<&InspectorCategory> {
        let mut v: Vec<&InspectorCategory> = self.categories.iter().collect();
        v.sort_by_key(|c| c.order);
        v
    }
}

/// Keep `current` if it appears in `applicable`; otherwise return the first
/// entry in `applicable`; if `applicable` is empty, return `current` unchanged.
/// `applicable` must be in strip order so the fallback is deterministic.
pub fn resolve_active_category<'a>(current: &'a str, applicable: &[&'a str]) -> &'a str {
    if applicable.contains(&current) {
        current
    } else {
        applicable.first().copied().unwrap_or(current)
    }
}

/// Seed the six built-in categories and their component type-path mappings.
pub fn seed_default_categories(r: &mut InspectorRegistry) {
    for (id, label, icon, order) in [
        ("object", "Object", Icon::Box, 0),
        ("mesh", "Mesh", Icon::Grid3x3, 10),
        ("modifiers", "Modifiers", Icon::Wrench, 20),
        ("material", "Material", Icon::Palette, 30),
        ("physics", "Physics", Icon::Orbit, 40),
        ("components", "Components", Icon::Component, 50),
    ] {
        r.register_category(InspectorCategory {
            id: id.into(),
            label: label.into(),
            icon,
            order,
        });
    }

    for (tp, cat) in [
        // Core transform/visibility components live in the Object tab.
        ("bevy_transform::components::transform::Transform", "object"),
        ("bevy_ecs::name::Name", "object"),
        ("bevy_render::view::visibility::Visibility", "object"),
        // Brush geometry lives in the Mesh tab. The real reflect path is
        // jackdaw_jsn::types::Brush; the short alias is seeded as well so
        // any lookup using the unqualified form also resolves correctly.
        ("jackdaw_jsn::types::Brush", "mesh"),
        ("jackdaw_jsn::Brush", "mesh"),
        // Modifier stack lives in the Modifiers tab.
        ("jackdaw_geometry::modifiers::ModifierStack", "modifiers"),
        // Standard material component lives in the Material tab.
        (
            "bevy_pbr::mesh_material::MeshMaterial3d<bevy_pbr::pbr_material::StandardMaterial>",
            "material",
        ),
        // Physics components.
        ("jackdaw_avian_integration::AvianCollider", "physics"),
        ("avian3d::dynamics::rigid_body::RigidBody", "physics"),
    ] {
        r.set_component_category(tp, cat);
    }

    // Namespace-prefix routing: any avian or integration path goes to physics.
    // Exact entries above take precedence (they are checked first in `category_for`).
    r.set_component_category_prefix("avian3d::", "physics");
    r.set_component_category_prefix("jackdaw_avian_integration::", "physics");
    // All `material_card::*` cards (Preview/Surface/Textures/Settings, and future
    // advanced cards) belong to the Material tab.
    r.set_component_category_prefix("material_card::", "material");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seeded() -> InspectorRegistry {
        let mut r = InspectorRegistry::default();
        seed_default_categories(&mut r);
        r
    }

    #[test]
    fn category_for_maps_builtins_and_defaults_to_components() {
        let r = seeded();
        assert_eq!(
            r.category_for("bevy_transform::components::transform::Transform"),
            "object"
        );
        assert_eq!(r.category_for("jackdaw_jsn::Brush"), "mesh");
        assert_eq!(r.category_for("some::unknown::Custom"), "components");
    }

    #[test]
    fn categories_sorted_by_order() {
        let r = seeded();
        let ids: Vec<&str> = r
            .categories_sorted()
            .iter()
            .map(|c| c.id.as_ref())
            .collect();
        assert_eq!(
            ids,
            [
                "object",
                "mesh",
                "modifiers",
                "material",
                "physics",
                "components"
            ]
        );
    }

    #[test]
    fn resolve_keeps_current_when_applicable_else_first() {
        assert_eq!(
            resolve_active_category("physics", &["object", "physics"]),
            "physics"
        );
        assert_eq!(
            resolve_active_category("physics", &["object", "mesh"]),
            "object"
        );
        assert_eq!(resolve_active_category("mesh", &[]), "mesh");
    }

    #[test]
    fn prefix_routing_and_exact_wins() {
        let r = seeded();
        // Internal avian path not seeded as exact goes to physics via prefix.
        assert_eq!(
            r.category_for("avian3d::dynamics::solver::SomeInternal"),
            "physics"
        );
        // Exact seed still resolves correctly.
        assert_eq!(
            r.category_for("avian3d::dynamics::rigid_body::RigidBody"),
            "physics"
        );
        // Unrelated path still falls back to components.
        assert_eq!(r.category_for("my_game::Health"), "components");
    }

    #[test]
    fn material_card_prefix_routes_to_material() {
        let mut r = InspectorRegistry::default();
        seed_default_categories(&mut r);
        assert_eq!(r.category_for("material_card::surface"), "material");
        assert_eq!(r.category_for("material_card::preview"), "material");
    }

    #[test]
    fn register_category_and_mapping() {
        let mut r = seeded();
        r.register_category(InspectorCategory {
            id: "gameplay".into(),
            label: "Gameplay".into(),
            icon: Icon::Gamepad2,
            order: 25,
        });
        r.set_component_category("my_game::Health", "gameplay");
        assert_eq!(r.category_for("my_game::Health"), "gameplay");
        let ids: Vec<&str> = r
            .categories_sorted()
            .iter()
            .map(|c| c.id.as_ref())
            .collect();
        assert_eq!(
            ids,
            [
                "object",
                "mesh",
                "modifiers",
                "gameplay",
                "material",
                "physics",
                "components"
            ]
        );
    }
}
