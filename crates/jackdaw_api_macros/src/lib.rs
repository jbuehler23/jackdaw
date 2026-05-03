//! Proc macros for `jackdaw_api`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{
    Expr, ExprLit, ExprPath, Ident, ItemFn, Lit, LitBool, LitStr, Meta, Path, Token, Visibility,
    parse_macro_input, punctuated::Punctuated, spanned::Spanned,
};

/// Marks a plain Bevy system function as an operator. Generates the
/// zero-sized action type, the `InputAction` derive, and the
/// `Operator` trait impl, leaving the function itself in place as
/// the `execute` system.
///
/// Required keys:
/// - `id`: the global operator id string
/// - `label`: human-readable label
///
/// Optional keys:
/// - `description`: long-form description (default `""`)
/// - `modal`: `bool`, default `false`
/// - `allows_undo`: `bool`, default `true`. When `false`, this operator never
///   creates an undo history entry.
/// - `is_available`: path to a Bevy system returning `bool` that
///   decides whether the operator can run in the current editor
///   state. Runs before the execute system on every `World::operator`
///   call and via `World::is_operator_available`. If it returns
///   `false`, the operator returns an error without executing.
/// - `cancel`: path to a Bevy system invoked when the operator is
///   cancelled.
/// - `name`: override the generated struct name. Default is
///   `PascalCase(fn_name) + "Op"`.
///
/// ```rust,ignore
/// use jackdaw_api::prelude::*;
///
/// fn time_is_running(time: Res<Time>) -> bool {
///     time.delta_secs_f32() > 0.0
/// }
///
/// #[operator(id = "sample.hello", label = "Hello", is_available = time_is_running)]
/// fn hello(_: In<OperatorParameters>) -> OperatorResult {
///     info!("hello");
///     OperatorResult::Finished
/// }
/// ```
///
/// Expands to a `HelloOp` struct with `InputAction` derived and an
/// `impl Operator for HelloOp` whose `register_execute` registers the
/// `hello` function as a Bevy system. When `is_available` is given,
/// `register_availability_check` is emitted too.
#[proc_macro_attribute]
pub fn operator(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(
        attr with Punctuated::<Meta, Token![,]>::parse_terminated
    );
    let mut item_fn = parse_macro_input!(item as ItemFn);

    let mut id: Option<Expr> = None;
    let mut label: Option<Expr> = None;
    let mut description: Option<Expr> = None;
    let mut modal: bool = false;
    let mut allows_undo: bool = true;
    let mut name_override: Option<String> = None;
    let mut is_available: Option<Path> = None;
    let mut cancel: Option<Path> = None;
    let mut params: Option<TokenStream2> = None;

    for arg in &args {
        match arg {
            Meta::NameValue(nv) => {
                let Some(key) = nv.path.get_ident().map(Ident::to_string) else {
                    continue;
                };
                match key.as_str() {
                    "id" => {
                        if let Some(s) = as_str_expr(&nv.value) {
                            id = Some(s);
                        }
                    }
                    "label" => {
                        if let Some(s) = as_str_expr(&nv.value) {
                            label = Some(s);
                        }
                    }
                    "description" => {
                        if let Some(s) = as_str_expr(&nv.value) {
                            description = Some(s);
                        }
                    }
                    "modal" => {
                        if let Some(b) = as_lit_bool(&nv.value) {
                            modal = b.value;
                        }
                    }
                    "allows_undo" => {
                        if let Some(b) = as_lit_bool(&nv.value) {
                            allows_undo = b.value;
                        }
                    }
                    "name" => {
                        if let Some(s) = as_lit_str(&nv.value) {
                            name_override = Some(s.value());
                        }
                    }
                    "is_available" => {
                        if let Some(p) = as_path(&nv.value) {
                            is_available = Some(p);
                        } else {
                            return syn::Error::new(
                                nv.value.span(),
                                "`is_available` must be the path of a Bevy system returning `bool`",
                            )
                            .into_compile_error()
                            .into();
                        }
                    }
                    "cancel" => {
                        if let Some(p) = as_path(&nv.value) {
                            cancel = Some(p);
                        } else {
                            return syn::Error::new(
                                nv.value.span(),
                                "`cancel` must be the path of a Bevy system",
                            )
                            .into_compile_error()
                            .into();
                        }
                    }
                    other => {
                        return syn::Error::new(
                            nv.path.span(),
                            format!("unknown `#[operator]` argument: `{other}`"),
                        )
                        .into_compile_error()
                        .into();
                    }
                }
            }
            Meta::List(list) if list.path.is_ident("params") => match build_params_const(list) {
                Ok(tokens) => params = Some(tokens),
                Err(err) => return err.into_compile_error().into(),
            },
            Meta::Path(path) if path.is_ident("modal") => {
                modal = true;
            }
            Meta::Path(path) if path.is_ident("allows_undo") => {
                allows_undo = true;
            }
            other => {
                return syn::Error::new(
                    other.span(),
                    "expected `key = value` or `params(...)` in `#[operator]`",
                )
                .into_compile_error()
                .into();
            }
        }
    }

    let Some(id) = id else {
        return syn::Error::new(Span::call_site(), "`#[operator]` requires `id = \"...\"`")
            .into_compile_error()
            .into();
    };
    let label = label.unwrap_or(id.clone());
    let description = description.unwrap_or_else(|| {
        Expr::Lit(ExprLit {
            lit: Lit::Str(LitStr::new("", Span::call_site())),
            attrs: vec![],
        })
    });

    let fn_name = &item_fn.sig.ident;
    let struct_name = match name_override {
        Some(n) => format_ident!("{}", n),
        None => format_ident!("{}Op", to_pascal_case(&fn_name.to_string())),
    };
    let vis = item_fn.vis.clone();
    item_fn.vis = Visibility::Inherited;

    let availability_impl = is_available.map(|path| {
        quote! {
            fn register_availability_check(
                commands: &mut ::bevy::ecs::system::Commands,
            ) -> ::core::option::Option<::bevy::ecs::system::SystemId<(), bool>> {
                ::core::option::Option::Some(commands.register_system(#path))
            }
        }
    });

    let cancel_impl = cancel.map(|path| {
        quote! {
            fn register_cancel(
                commands: &mut ::bevy::ecs::system::Commands,
            ) -> ::core::option::Option<::bevy::ecs::system::SystemId<()>> {
                ::core::option::Option::Some(commands.register_system(#path))
            }
        }
    });

    let parameters_const = params.map(|tokens| {
        quote! {
            const PARAMETERS: &'static [::jackdaw_api::prelude::ParamSpec] = #tokens;
        }
    });

    let expanded = quote! {
        #[derive(::core::default::Default, ::bevy_enhanced_input::prelude::InputAction)]
        #[action_output(bool)]
        #vis struct #struct_name;

        impl ::jackdaw_api::prelude::Operator for #struct_name {
            const ID: &'static str = #id;
            const LABEL: &'static str = #label;
            const DESCRIPTION: &'static str = #description;
            const MODAL: bool = #modal;
            const ALLOWS_UNDO: bool = #allows_undo;

            #parameters_const

            fn register_execute(
                commands: &mut ::bevy::ecs::system::Commands,
            ) -> ::jackdaw_api::prelude::OperatorSystemId {
                commands.register_system(#fn_name)
            }

            #availability_impl

            #cancel_impl
        }

        #item_fn
    };

    expanded.into()
}

fn as_lit_str(expr: &Expr) -> Option<LitStr> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Str(s), ..
    }) = expr
    {
        Some(s.clone())
    } else {
        None
    }
}

fn as_str_expr(expr: &Expr) -> Option<Expr> {
    match expr {
        Expr::Lit(ExprLit {
            lit: Lit::Str(_), ..
        })
        | Expr::Path(_) => Some(expr.clone()),

        _ => None,
    }
}

fn as_lit_bool(expr: &Expr) -> Option<LitBool> {
    if let Expr::Lit(ExprLit {
        lit: Lit::Bool(b), ..
    }) = expr
    {
        Some(b.clone())
    } else {
        None
    }
}

fn as_path(expr: &Expr) -> Option<Path> {
    if let Expr::Path(ExprPath { path, .. }) = expr {
        Some(path.clone())
    } else {
        None
    }
}

fn to_pascal_case(snake: &str) -> String {
    let mut out = String::with_capacity(snake.len());
    for part in snake.split('_') {
        let mut chars = part.chars();
        if let Some(c) = chars.next() {
            out.extend(c.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

/// Lower a `params(name(Type, default = ..., doc = "..."), ...)` block into
/// the const slice expression that goes after `PARAMETERS: &'static [ParamSpec] =`.
fn build_params_const(list: &syn::MetaList) -> syn::Result<TokenStream2> {
    let entries: Punctuated<Meta, Token![,]> =
        list.parse_args_with(Punctuated::parse_terminated)?;
    let mut items = Vec::with_capacity(entries.len());
    for entry in &entries {
        items.push(build_param_spec(entry)?);
    }
    Ok(quote! { &[ #( #items ),* ] })
}

fn build_param_spec(meta: &Meta) -> syn::Result<TokenStream2> {
    let Meta::List(list) = meta else {
        return Err(syn::Error::new(
            meta.span(),
            "expected `name(Type, default = ..., doc = \"...\")`",
        ));
    };
    let name_ident = list.path.get_ident().ok_or_else(|| {
        syn::Error::new(
            list.path.span(),
            "parameter name must be a simple identifier",
        )
    })?;
    let name_lit = LitStr::new(&name_ident.to_string(), name_ident.span());

    let inner: Punctuated<Meta, Token![,]> = list.parse_args_with(Punctuated::parse_terminated)?;
    let mut iter = inner.iter();

    let ty_meta = iter
        .next()
        .ok_or_else(|| syn::Error::new(list.span(), "parameter is missing a type (e.g. `i64`)"))?;
    let ty_ident = match ty_meta {
        Meta::Path(path) => path.get_ident().cloned().ok_or_else(|| {
            syn::Error::new(path.span(), "parameter type must be a single identifier")
        })?,
        _ => {
            return Err(syn::Error::new(
                ty_meta.span(),
                "parameter type must be the first argument and a single identifier",
            ));
        }
    };
    let ty_variant = param_type_variant(&ty_ident)?;

    let mut default_expr: Option<Expr> = None;
    let mut doc_lit: Option<LitStr> = None;
    for m in iter {
        let Meta::NameValue(nv) = m else {
            return Err(syn::Error::new(
                m.span(),
                "expected `default = ...` or `doc = \"...\"`",
            ));
        };
        let key = nv.path.get_ident().ok_or_else(|| {
            syn::Error::new(nv.path.span(), "parameter attribute must be an ident")
        })?;
        match key.to_string().as_str() {
            "default" => default_expr = Some(nv.value.clone()),
            "doc" => {
                let lit = as_lit_str(&nv.value).ok_or_else(|| {
                    syn::Error::new(nv.value.span(), "`doc` must be a string literal")
                })?;
                doc_lit = Some(lit);
            }
            other => {
                return Err(syn::Error::new(
                    key.span(),
                    format!("unknown parameter attribute: `{other}`"),
                ));
            }
        }
    }

    let default_tokens = match &default_expr {
        Some(expr) => {
            let variant = param_default_variant(&ty_ident, expr)?;
            quote! { ::core::option::Option::Some(#variant) }
        }
        None => quote! { ::core::option::Option::None },
    };
    let doc_tokens = match &doc_lit {
        Some(lit) => quote! { #lit },
        None => quote! { "" },
    };

    Ok(quote! {
        ::jackdaw_api::prelude::ParamSpec {
            name: #name_lit,
            ty: #ty_variant,
            default: #default_tokens,
            doc: #doc_tokens,
        }
    })
}

/// Validate the parameter type ident and return the matching
/// title-case label as a string literal token. The literal is what
/// `ParamSpec.ty` stores; it matches the labels produced by
/// `jackdaw_jsn::PropertyValue::type_name`.
fn param_type_variant(ty: &Ident) -> syn::Result<TokenStream2> {
    let label = match ty.to_string().as_str() {
        "bool" => "Bool",
        "i64" => "Int",
        "f64" => "Float",
        "String" => "String",
        "Vec2" => "Vec2",
        "Vec3" => "Vec3",
        "Color" => "Color",
        "Entity" => "Entity",
        other => {
            return Err(syn::Error::new(
                ty.span(),
                format!(
                    "unknown parameter type `{other}` (expected bool, i64, f64, String, Vec2, Vec3, Color, Entity)",
                ),
            ));
        }
    };
    Ok(quote! { #label })
}

/// Lower a `default = ...` macro arg into a `PropertyValue` constructor.
/// Strings go through `Cow::Borrowed` so the whole `ParamSpec` can sit
/// in a `const` slice; numeric and bool literals are trivial.
fn param_default_variant(ty: &Ident, expr: &Expr) -> syn::Result<TokenStream2> {
    Ok(match ty.to_string().as_str() {
        "bool" => quote! { ::jackdaw_api::jsn::PropertyValue::Bool(#expr) },
        "i64" => quote! { ::jackdaw_api::jsn::PropertyValue::Int(#expr) },
        "f64" => quote! { ::jackdaw_api::jsn::PropertyValue::Float(#expr) },
        "String" => quote! {
            ::jackdaw_api::jsn::PropertyValue::String(::std::borrow::Cow::Borrowed(#expr))
        },
        "Vec2" | "Vec3" | "Color" | "Entity" => {
            return Err(syn::Error::new(
                expr.span(),
                "literal `default = ...` for Vec2, Vec3, Color, Entity is not supported yet",
            ));
        }
        other => {
            return Err(syn::Error::new(
                ty.span(),
                format!("unknown parameter type `{other}` for default lowering"),
            ));
        }
    })
}
