use std::path::Path;

use bevy::asset::{AssetServer, ReflectHandle};
use bevy::platform::collections::HashMap;
use bevy::reflect::enums::VariantType;
use bevy::reflect::{PartialReflect, ReflectRef, TypeRegistry};

use super::{BsnField, BsnPatch, BsnStructData, BsnStructFields, BsnTupleStructData, BsnValue};

/// Context for resolving `Handle<T>` fields to asset-path strings during BSN
/// emission. `parent_path` is the directory the emitted `.bsn` file lives in,
/// used to make emitted asset paths relative. `asset_names` maps assets that
/// have no filesystem path (catalog and scene-inline assets) to their
/// reference names (`@Name` or `#Name`), which are emitted verbatim.
pub struct BsnAssetContext<'a> {
    pub asset_server: &'a AssetServer,
    pub parent_path: &'a Path,
    pub asset_names: Option<&'a HashMap<bevy::asset::UntypedAssetId, String>>,
}

impl BsnAssetContext<'_> {
    /// Resolve a handle to its emitted string: a (relative) asset path when the
    /// asset came from disk, else its catalog/inline reference name, else an
    /// empty string.
    fn handle_string(&self, id: bevy::asset::UntypedAssetId) -> String {
        if let Some(path) = self.asset_server.get_path(id) {
            let path_str = path.to_string();
            if let Some(relative) = pathdiff::diff_paths(&path_str, self.parent_path) {
                return relative.to_string_lossy().into_owned();
            }
            return path_str;
        }
        if let Some(names) = self.asset_names
            && let Some(name) = names.get(&id)
        {
            return name.clone();
        }
        String::new()
    }
}

/// Whether a reflect type path denotes a `bevy_asset` `Handle<T>`. Used to
/// short-circuit handles whose asset type is not registered with
/// `ReflectHandle`, so the reflect walk never descends into their `AssetId`.
fn is_handle_type_path(type_path: &str) -> bool {
    type_path.starts_with("bevy_asset::handle::Handle<")
}

impl BsnValue {
    /// Create a [`BsnValue`] from a reflected value and its type info.
    pub fn from_reflect(value: &dyn PartialReflect, type_registry: &TypeRegistry) -> Self {
        Self::from_reflect_inner(value, type_registry, None)
    }

    /// Create a [`BsnValue`] from a reflected value, resolving `Handle<T>`
    /// fields (at any nesting depth) to asset-path strings via `ctx`.
    pub fn from_reflect_with_assets(
        value: &dyn PartialReflect,
        type_registry: &TypeRegistry,
        ctx: &BsnAssetContext,
    ) -> Self {
        Self::from_reflect_inner(value, type_registry, Some(ctx))
    }

    fn from_reflect_inner(
        value: &dyn PartialReflect,
        type_registry: &TypeRegistry,
        ctx: Option<&BsnAssetContext>,
    ) -> Self {
        // Try primitives first.
        if let Some(v) = value.try_downcast_ref::<f32>() {
            return BsnValue::Float(*v as f64);
        }
        if let Some(v) = value.try_downcast_ref::<f64>() {
            return BsnValue::Float(*v);
        }
        if let Some(v) = value.try_downcast_ref::<bool>() {
            return BsnValue::Bool(*v);
        }
        if let Some(v) = value.try_downcast_ref::<String>() {
            return BsnValue::String(v.clone());
        }
        if let Some(v) = value.try_downcast_ref::<std::borrow::Cow<'static, str>>() {
            return BsnValue::String(v.to_string());
        }
        // Path types reflect as opaque and would otherwise hit the Debug
        // fallback, which wraps the path in literal quotes. Emit the plain
        // path string so it round-trips through one string value.
        if let Some(v) = value.try_downcast_ref::<std::path::PathBuf>() {
            return BsnValue::String(v.to_string_lossy().into_owned());
        }
        // Integer types.
        if let Some(v) = value.try_downcast_ref::<i32>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u32>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i64>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u64>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<usize>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i8>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u8>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<i16>() {
            return BsnValue::Int(*v as i128);
        }
        if let Some(v) = value.try_downcast_ref::<u16>() {
            return BsnValue::Int(*v as i128);
        }

        // Handle<T> fields: resolve to an asset-path string when an asset
        // context is available. Without a context a handle has no stable text
        // form, so emit an empty string rather than descending into its
        // `AssetId` (whose `Uuid` / `Index` has no BSN representation and only
        // produces "no BSN representation" noise on every sync). This is
        // recognised regardless of `ctx` so a context-free sync (e.g. the live
        // brush-confirm mirror, whose material handles are runtime-only) does
        // not fall through to the reflect walk.
        if let Some(concrete) = value.try_as_reflect() {
            let type_id = concrete.reflect_type_info().type_id();
            if let Some(reflect_handle) = type_registry.get_type_data::<ReflectHandle>(type_id) {
                if let Some(ctx) = ctx
                    && let Some(untyped_handle) =
                        reflect_handle.downcast_handle_untyped(concrete.as_any())
                {
                    return BsnValue::String(ctx.handle_string(untyped_handle.id()));
                }
                // No context (or a handle that failed to downcast): emit an
                // empty string.
                return BsnValue::String(String::new());
            }
        }

        // A `Handle<T>` whose asset type is not registered with
        // `ReflectHandle` (no `register_asset_reflect`) misses the branch
        // above, and the reflect walk would then descend into the handle
        // enum's `AssetId`, whose `Uuid` / `Index` have no BSN form and only
        // produce "no BSN representation" noise on every world->BSN sync.
        // Recognise any handle by its type path and drop it to an empty
        // string, the same graceful degradation the registered-but-
        // unresolvable case above uses.
        if value
            .get_represented_type_info()
            .is_some_and(|info| is_handle_type_path(info.type_path()))
        {
            return BsnValue::String(String::new());
        }

        // Option<Handle<T>> fields: emit the inner asset path (Some) or an
        // empty string (None) so the value round-trips through one path string.
        if let Some(ctx) = ctx
            && let ReflectRef::Enum(e) = value.reflect_ref()
        {
            let inner_is_handle = value
                .get_represented_type_info()
                .and_then(|info| match info {
                    bevy::reflect::TypeInfo::Enum(enum_info)
                        if enum_info.type_path().starts_with("core::option::Option<") =>
                    {
                        enum_info.variant("Some")
                    }
                    _ => None,
                })
                .and_then(|variant| match variant {
                    bevy::reflect::enums::VariantInfo::Tuple(tuple) => tuple.field_at(0),
                    _ => None,
                })
                .map(|field| {
                    type_registry
                        .get_type_data::<ReflectHandle>(field.type_id())
                        .is_some()
                })
                .unwrap_or(false);

            if inner_is_handle {
                match e.variant_name() {
                    "None" => return BsnValue::String(String::new()),
                    "Some" => {
                        if let Some(inner) = e.field_at(0)
                            && let Some(concrete) = inner.try_as_reflect()
                        {
                            let type_id = concrete.reflect_type_info().type_id();
                            if let Some(reflect_handle) =
                                type_registry.get_type_data::<ReflectHandle>(type_id)
                                && let Some(untyped_handle) =
                                    reflect_handle.downcast_handle_untyped(concrete.as_any())
                            {
                                return BsnValue::String(ctx.handle_string(untyped_handle.id()));
                            }
                        }
                    }
                    _ => {}
                }
            }
        }

        // Structs.
        if let ReflectRef::Struct(s) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let mut fields = Vec::new();
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap().to_string();
                let field_value = s.field_at(i).unwrap();
                fields.push(BsnField {
                    name,
                    value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                });
            }
            return BsnValue::Struct(BsnStructData {
                type_path,
                fields: BsnStructFields(fields),
            });
        }

        // Tuple structs.
        if let ReflectRef::TupleStruct(ts) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let mut values = Vec::new();
            for i in 0..ts.field_len() {
                let field_value = ts.field(i).unwrap();
                values.push(BsnValue::from_reflect_inner(
                    field_value,
                    type_registry,
                    ctx,
                ));
            }
            return BsnValue::TupleStruct(BsnTupleStructData { type_path, values });
        }

        // Enums.
        if let ReflectRef::Enum(e) = value.reflect_ref() {
            let type_path = value
                .get_represented_type_info()
                .map(|info| info.type_path().to_string())
                .unwrap_or_default();
            let variant = e.variant_name();
            let full_path = format!("{type_path}::{variant}");
            match e.variant_type() {
                VariantType::Struct => {
                    let mut fields = Vec::new();
                    for i in 0..e.field_len() {
                        let name = e.name_at(i).unwrap().to_string();
                        let field_value = e.field_at(i).unwrap();
                        fields.push(BsnField {
                            name,
                            value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                        });
                    }
                    return BsnValue::Struct(BsnStructData {
                        type_path: full_path,
                        fields: BsnStructFields(fields),
                    });
                }
                VariantType::Tuple => {
                    let mut values = Vec::new();
                    for i in 0..e.field_len() {
                        let field_value = e.field_at(i).unwrap();
                        values.push(BsnValue::from_reflect_inner(
                            field_value,
                            type_registry,
                            ctx,
                        ));
                    }
                    return BsnValue::TupleStruct(BsnTupleStructData {
                        type_path: full_path,
                        values,
                    });
                }
                VariantType::Unit => {
                    return BsnValue::Type(full_path);
                }
            }
        }

        // Lists / Vecs.
        if let ReflectRef::List(l) = value.reflect_ref() {
            let mut items = Vec::new();
            for i in 0..l.len() {
                if let Some(item) = l.get(i) {
                    items.push(BsnValue::from_reflect_inner(item, type_registry, ctx));
                }
            }
            return BsnValue::List(items);
        }

        // Fixed-size arrays emit like lists; the applier rebuilds them against
        // the concrete array type.
        if let ReflectRef::Array(a) = value.reflect_ref() {
            let mut items = Vec::new();
            for i in 0..a.len() {
                if let Some(item) = a.get(i) {
                    items.push(BsnValue::from_reflect_inner(item, type_registry, ctx));
                }
            }
            return BsnValue::List(items);
        }

        // Maps / HashMaps.
        if let ReflectRef::Map(m) = value.reflect_ref() {
            let mut entries = Vec::new();
            for (key, val) in m.iter() {
                entries.push((
                    BsnValue::from_reflect_inner(key, type_registry, ctx),
                    BsnValue::from_reflect_inner(val, type_registry, ctx),
                ));
            }
            return BsnValue::Map(entries);
        }

        // Fallback: emit as string via Debug. The quoted string parses back,
        // but the original typed value is not recoverable from it.
        log::warn!(
            "value of type '{}' has no BSN representation; storing its Debug form as a string",
            value
                .get_represented_type_info()
                .map(bevy::reflect::TypeInfo::type_path)
                .unwrap_or("<unknown>")
        );
        BsnValue::String(format!("{value:?}"))
    }
}

/// Convert a component's reflected data into a BSN AST patch.
pub fn component_to_bsn_patch(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
) -> BsnPatch {
    component_to_bsn_patch_inner(reflected, type_registry, None)
}

/// Convert a component's reflected data into a BSN AST patch, resolving
/// `Handle<T>` fields (at any nesting depth) to asset-path strings via `ctx`.
pub fn component_to_bsn_patch_with_assets(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
    ctx: &BsnAssetContext,
) -> BsnPatch {
    component_to_bsn_patch_inner(reflected, type_registry, Some(ctx))
}

fn component_to_bsn_patch_inner(
    reflected: &dyn PartialReflect,
    type_registry: &TypeRegistry,
    ctx: Option<&BsnAssetContext>,
) -> BsnPatch {
    use bevy::reflect::prelude::ReflectDefault;

    let type_path = reflected
        .get_represented_type_info()
        .map(|info| info.type_path().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    match reflected.reflect_ref() {
        ReflectRef::Struct(s) => {
            let reg = type_registry.get_with_type_path(&type_path);
            let default_instance = reg
                .and_then(|r| r.data::<ReflectDefault>())
                .map(ReflectDefault::default);

            let mut fields = Vec::new();
            for i in 0..s.field_len() {
                let name = s.name_at(i).unwrap().to_string();
                let field_value = s.field_at(i).unwrap();

                // Only emit fields that differ from default.
                let should_emit = if let Some(ref default) = default_instance {
                    if let ReflectRef::Struct(ds) = default.reflect_ref() {
                        if let Some(default_field) = ds.field(&name) {
                            !field_value
                                .reflect_partial_eq(default_field)
                                .unwrap_or(false)
                        } else {
                            true
                        }
                    } else {
                        true
                    }
                } else {
                    true
                };

                if should_emit {
                    fields.push(BsnField {
                        name,
                        value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                    });
                }
            }

            if fields.is_empty() {
                BsnPatch::Type(type_path)
            } else {
                BsnPatch::Struct(BsnStructData {
                    type_path,
                    fields: BsnStructFields(fields),
                })
            }
        }

        ReflectRef::TupleStruct(ts) => {
            let mut values = Vec::new();
            for i in 0..ts.field_len() {
                let field_value = ts.field(i).unwrap();
                values.push(BsnValue::from_reflect_inner(
                    field_value,
                    type_registry,
                    ctx,
                ));
            }
            BsnPatch::TupleStruct(BsnTupleStructData { type_path, values })
        }

        ReflectRef::Enum(e) => {
            let variant = e.variant_name();
            let full_path = format!("{type_path}::{variant}");
            match e.variant_type() {
                VariantType::Struct => {
                    let mut fields = Vec::new();
                    for i in 0..e.field_len() {
                        let name = e.name_at(i).unwrap().to_string();
                        let field_value = e.field_at(i).unwrap();
                        fields.push(BsnField {
                            name,
                            value: BsnValue::from_reflect_inner(field_value, type_registry, ctx),
                        });
                    }
                    if fields.is_empty() {
                        BsnPatch::Type(full_path)
                    } else {
                        BsnPatch::Struct(BsnStructData {
                            type_path: full_path,
                            fields: BsnStructFields(fields),
                        })
                    }
                }
                VariantType::Tuple => {
                    let mut values = Vec::new();
                    for i in 0..e.field_len() {
                        let field_value = e.field_at(i).unwrap();
                        values.push(BsnValue::from_reflect_inner(
                            field_value,
                            type_registry,
                            ctx,
                        ));
                    }
                    BsnPatch::TupleStruct(BsnTupleStructData {
                        type_path: full_path,
                        values,
                    })
                }
                VariantType::Unit => BsnPatch::Type(full_path),
            }
        }

        _ => BsnPatch::Type(type_path),
    }
}
