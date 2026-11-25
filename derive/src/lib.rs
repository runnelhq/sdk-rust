//! Derive macro `Config` of the crate `runnel-plugin`: the JSON Schema of a configuration
//! from a Rust struct, the same way Runnel derives the schemas of its built-in actions from
//! Go structs (ADR 0014 of Runnel). Use it through `runnel_plugin::Config`; the
//! documentation of the attributes is there.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Attribute, Data, DeriveInput, Expr, Fields, Lit, LitStr, Meta, parse_macro_input, spanned::Spanned};

/// Derives `runnel_plugin::Config` (and `runnel_plugin::schema::SchemaType`) for a struct
/// with named fields. See the documentation of `runnel_plugin::Config`.
#[proc_macro_derive(Config, attributes(runnel))]
pub fn derive_config(input: TokenStream) -> TokenStream {
    let input = parse_macro_input!(input as DeriveInput);
    match expand(&input) {
        Ok(ts) => ts.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

#[derive(Default)]
struct FieldOptions {
    doc: Option<String>,
    required: bool,
    default: Option<String>,
    example: Option<String>,
    enumeration: Option<String>,
    format: Option<String>,
    group: Option<String>,
    only: Option<String>,
    connection: Option<String>,
    skip: bool,
}

struct SerdeField {
    rename: Option<String>,
    skip: bool,
    flatten: bool,
}

fn expand(input: &DeriveInput) -> syn::Result<TokenStream2> {
    let name = &input.ident;
    let Data::Struct(data) = &input.data else {
        return Err(syn::Error::new(
            input.span(),
            "Config can only be derived for structs with named fields",
        ));
    };
    let fields = match &data.fields {
        Fields::Named(f) => f.named.iter().collect::<Vec<_>>(),
        Fields::Unit => Vec::new(),
        Fields::Unnamed(_) => {
            return Err(syn::Error::new(
                input.span(),
                "Config can only be derived for structs with named fields",
            ));
        }
    };
    let rename_all = serde_rename_all(&input.attrs)?;
    let mut steps = Vec::new();
    for f in fields {
        let ident = f.ident.as_ref().expect("named field");
        let serde = serde_field(&f.attrs)?;
        let opts = runnel_options(&f.attrs)?;
        let ty = &f.ty;
        if serde.skip || opts.skip {
            continue;
        }
        if serde.flatten {
            steps.push(quote! {
                builder.flatten(<#ty as ::runnel_plugin::Config>::schema_at(builder.depth())?)?;
            });
            continue;
        }
        let raw = ident.to_string();
        let raw = raw.strip_prefix("r#").unwrap_or(&raw).to_string();
        let field_name = match serde.rename {
            Some(r) => r,
            None => apply_rename_all(&raw, rename_all.as_deref()),
        };
        let doc = opts.doc.clone().or_else(|| doc_comment(&f.attrs)).unwrap_or_default();
        let opt = |o: &Option<String>| match o {
            Some(s) => quote!(::core::option::Option::Some(#s)),
            None => quote!(::core::option::Option::None),
        };
        let (default, example, enumeration, format, group, only, connection) = (
            opt(&opts.default),
            opt(&opts.example),
            opt(&opts.enumeration),
            opt(&opts.format),
            opt(&opts.group),
            opt(&opts.only),
            opt(&opts.connection),
        );
        let required = opts.required;
        // A connection field names the connection in the file; the plugin receives it
        // resolved, so the Rust type of the field is not part of the schema.
        let schema = if opts.connection.is_some() {
            quote!(::runnel_plugin::schema::ConfigSchema::of_type("string"))
        } else {
            quote!(<#ty as ::runnel_plugin::schema::SchemaType>::schema_at(builder.depth())?)
        };
        steps.push(quote! {
            builder.field(#field_name, #schema, ::runnel_plugin::schema::FieldOptions {
                doc: #doc,
                required: #required,
                default: #default,
                example: #example,
                enumeration: #enumeration,
                format: #format,
                group: #group,
                only: #only,
                connection: #connection,
            })?;
        });
    }
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        impl #impl_generics ::runnel_plugin::Config for #name #ty_generics #where_clause {
            fn schema_at(depth: usize) -> ::core::result::Result<::runnel_plugin::schema::ConfigSchema, ::runnel_plugin::schema::SchemaError> {
                let mut builder = ::runnel_plugin::schema::ObjectBuilder::new(depth)?;
                #(#steps)*
                builder.finish()
            }
        }
        impl #impl_generics ::runnel_plugin::schema::SchemaType for #name #ty_generics #where_clause {
            fn schema_at(depth: usize) -> ::core::result::Result<::runnel_plugin::schema::ConfigSchema, ::runnel_plugin::schema::SchemaError> {
                <Self as ::runnel_plugin::Config>::schema_at(depth)
            }
        }
    })
}

fn doc_comment(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for a in attrs {
        if !a.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(nv) = &a.meta
            && let Expr::Lit(l) = &nv.value
            && let Lit::Str(s) = &l.lit
        {
            lines.push(s.value().trim().to_string());
        }
    }
    let text = lines
        .into_iter()
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn runnel_options(attrs: &[Attribute]) -> syn::Result<FieldOptions> {
    let mut o = FieldOptions::default();
    for a in attrs {
        if !a.path().is_ident("runnel") {
            continue;
        }
        a.parse_nested_meta(|meta| {
            let key = meta.path.get_ident().map(|i| i.to_string()).unwrap_or_default();
            let value = |meta: &syn::meta::ParseNestedMeta| -> syn::Result<String> {
                let s: LitStr = meta.value()?.parse()?;
                Ok(s.value())
            };
            match key.as_str() {
                "required" => o.required = true,
                "skip" => o.skip = true,
                "doc" => o.doc = Some(value(&meta)?),
                "default" => o.default = Some(value(&meta)?),
                "example" => o.example = Some(value(&meta)?),
                "values" => o.enumeration = Some(value(&meta)?),
                "format" => o.format = Some(value(&meta)?),
                "group" => o.group = Some(value(&meta)?),
                "only" => o.only = Some(value(&meta)?),
                "connection" => o.connection = Some(value(&meta)?),
                _ => {
                    return Err(meta.error(
                        "unknown runnel option; known are required, skip, doc, default, example, values, format, group, only, connection",
                    ));
                }
            }
            Ok(())
        })?;
    }
    Ok(o)
}

fn serde_field(attrs: &[Attribute]) -> syn::Result<SerdeField> {
    let mut f = SerdeField {
        rename: None,
        skip: false,
        flatten: false,
    };
    for a in attrs {
        if !a.path().is_ident("serde") {
            continue;
        }
        a.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                if meta.input.peek(syn::Token![=]) {
                    let s: LitStr = meta.value()?.parse()?;
                    f.rename = Some(s.value());
                } else {
                    // rename(deserialize = "…", serialize = "…")
                    meta.parse_nested_meta(|inner| {
                        let s: LitStr = inner.value()?.parse()?;
                        if inner.path.is_ident("deserialize") {
                            f.rename = Some(s.value());
                        }
                        Ok(())
                    })?;
                }
            } else if meta.path.is_ident("skip") || meta.path.is_ident("skip_deserializing") {
                f.skip = true;
            } else if meta.path.is_ident("flatten") {
                f.flatten = true;
            } else if meta.input.peek(syn::Token![=]) {
                let _: Expr = meta.value()?.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let _content;
                syn::parenthesized!(_content in meta.input);
            }
            Ok(())
        })?;
    }
    Ok(f)
}

fn serde_rename_all(attrs: &[Attribute]) -> syn::Result<Option<String>> {
    let mut rule = None;
    for a in attrs {
        if !a.path().is_ident("serde") {
            continue;
        }
        a.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                if meta.input.peek(syn::Token![=]) {
                    let s: LitStr = meta.value()?.parse()?;
                    rule = Some(s.value());
                } else {
                    meta.parse_nested_meta(|inner| {
                        let s: LitStr = inner.value()?.parse()?;
                        if inner.path.is_ident("deserialize") {
                            rule = Some(s.value());
                        }
                        Ok(())
                    })?;
                }
            } else if meta.input.peek(syn::Token![=]) {
                let _: Expr = meta.value()?.parse()?;
            } else if meta.input.peek(syn::token::Paren) {
                let _content;
                syn::parenthesized!(_content in meta.input);
            }
            Ok(())
        })?;
    }
    Ok(rule)
}

/// Applies serde's rename_all rules to the snake_case name of a field.
fn apply_rename_all(field: &str, rule: Option<&str>) -> String {
    let words: Vec<&str> = field.split('_').filter(|w| !w.is_empty()).collect();
    let capitalize = |w: &str| {
        let mut c = w.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    match rule {
        Some("lowercase") => field.to_lowercase(),
        Some("UPPERCASE") => field.to_uppercase(),
        Some("camelCase") => words
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { w.to_string() } else { capitalize(w) })
            .collect(),
        Some("PascalCase") => words.iter().map(|w| capitalize(w)).collect(),
        Some("SCREAMING_SNAKE_CASE") => field.to_uppercase(),
        Some("kebab-case") => field.replace('_', "-"),
        Some("SCREAMING-KEBAB-CASE") => field.replace('_', "-").to_uppercase(),
        _ => field.to_string(),
    }
}
