use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Type};

struct FieldConfig {
    filter: Option<Expr>,
    map: Option<(Expr, Type)>, // (The map closure, Extracted source type)
    each: bool,
}

/// Helper function to extract the explicitly annotated argument type from a closure.
/// For example, `|x: Bar| ...` will return the AST representation of `Bar`.
fn extract_source_type_from_closure(expr: &Expr) -> syn::Result<Type> {
    if let Expr::Closure(closure) = expr {
        if let Some(syn::Pat::Type(pat_type)) = closure.inputs.first() {
            return Ok(*pat_type.ty.clone());
        }
    }
    Err(syn::Error::new_spanned(
        expr, 
        "The `map` attribute requires a closure with an explicitly typed argument. \n\
        Example: `#[resolve(map = |x: SourceType| ...)]`"
    ))
}

fn parse_resolve_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut config = FieldConfig { filter: None, map: None, each: false };
    for attr in attrs {
        if attr.path().is_ident("resolve") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("filter") {
                    config.filter = Some(meta.value()?.parse()?);
                    Ok(())
                } else if meta.path.is_ident("map") {
                    let expr: Expr = meta.value()?.parse()?;
                    let src_ty = extract_source_type_from_closure(&expr)?;
                    config.map = Some((expr, src_ty));
                    Ok(())
                } else if meta.path.is_ident("each") {
                    config.each = true;
                    Ok(())
                } else {
                    Err(meta.error("unsupported resolve attribute"))
                }
            })?;
        }
    }
    Ok(config)
}

#[proc_macro_derive(Resolve, attributes(resolve))]
pub fn derive_resolve(input: TokenStream) -> TokenStream {
    // Parse input tokens into a syntax tree
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    
    // Support generics
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the body of the `iter` function based on the struct's fields
    let resolve_body = match input.data {
        // Structs with named fields: struct Foo { bar: Bar }
        Data::Struct(ref data_struct) => match data_struct.fields {
            Fields::Named(ref fields) => {
                let mut field_names = Vec::new();
                let mut non_each_inits = Vec::new();
                let mut each_loops = Vec::new();

                for field in fields.named.iter() {
                    let field_name = field.ident.as_ref().unwrap();
                    let attrs = parse_resolve_attrs(&field.attrs).unwrap_or_else(|e| {
                        panic!("Failed to parse attributes for field '{}': {}", field_name, e)
                    });
                    
                    let ty = &field.ty;
                    field_names.push(field_name);

                    // 1. Determine base iterator call and any map transformations
                    let (base_iter_call, map_step) = match &attrs.map {
                        Some((map_expr, src_ty)) => {
                            (
                                quote! { <#src_ty as Resolve>::iter(session)? }, 
                                quote! { let __iter = std::iter::Iterator::map(__iter, #map_expr); }
                            )
                        }
                        None => {
                            (
                                quote! { <<#ty as Resolve>::Item as Resolve>::iter(session)? }, 
                                quote! {}
                            )
                        }
                    };

                    // 2. Determine filter transformations (Applies BEFORE map if both are present)
                    let filter_step = match &attrs.filter {
                        Some(f) => quote! { let __iter = std::iter::Iterator::filter(__iter, #f); },
                        None => quote! {},
                    };

                    let needs_custom_iter = attrs.map.is_some() || attrs.filter.is_some();

                    // 3. Construct the resolved iterator expression
                    let iter_expr = if needs_custom_iter {
                        quote! {
                            {
                                let __iter = #base_iter_call;
                                #filter_step
                                #map_step
                                <#ty as Resolve>::iter_from_items(__iter)?
                            }
                        }
                    } else {
                        quote! { <#ty as Resolve>::iter(session)? }
                    };

                    if attrs.each {
                        each_loops.push((field_name, iter_expr));
                    } else {
                        // Standard field logic
                        let init_tokens = if needs_custom_iter {
                            quote! {
                                {
                                    let mut __field_iter = #iter_expr;
                                    std::iter::Iterator::next(&mut __field_iter)
                                        .ok_or_else(|| ResolveDependencyError::DependencyNotAvailable(
                                            std::any::type_name::<#ty>().to_string()
                                        ))?
                                }
                            }
                        } else {
                            quote! { <#ty as Resolve>::first(session)? }
                        };

                        non_each_inits.push(quote! { let #field_name = #init_tokens; });
                    }
                }

                // Determine which fields need to be cloned
                let mut struct_inits = Vec::new();
                for field_name in &field_names {
                    let is_each = each_loops.iter().any(|(n, _)| n == field_name);
                    let is_last_each = each_loops.last().map(|(n, _)| *n) == Some(field_name);
                    
                    if is_each && !is_last_each {
                        // Outer `each` fields must be cloned for the inner loops!
                        struct_inits.push(quote! { #field_name: #field_name.clone() });
                    } else {
                        // Standard fields and the innermost `each` field do not need cloning
                        struct_inits.push(quote! { #field_name });
                    }
                }

                // Build the final struct configuration
                if each_loops.is_empty() {
                    quote! {
                        #( #non_each_inits )*
                        let __items = std::iter::once(Self {
                            #( #struct_inits ),*
                        });
                        
                        Self::iter_from_items(__items)
                    }
                } else {
                    let mut inner_block = quote! {
                        #( #non_each_inits )*
                        results.push(Self {
                            #( #struct_inits ),*
                        });
                    };

                    for (name, iter_expr) in each_loops.into_iter().rev() {
                        inner_block = quote! {
                            for #name in #iter_expr {
                                #inner_block
                            }
                        };
                    }

                    quote! {
                        let mut results = std::vec::Vec::new();
                        #inner_block
                        Self::iter_from_items(results.into_iter())
                    }
                }
            }
            // Tuple structs: struct Foo(Bar, Baz)
            Fields::Unnamed(ref _fields) => {
                quote! {
                    compile_error!("Currently only named structs are supported in this POC.");
                }
            }
            // Unit structs: struct Bar;
            Fields::Unit => {
                quote! {
                    let __items = std::iter::once(Self);
                    Self::iter_from_items(__items)
                }
            }
        },
        _ => quote! { compile_error!("Resolve can only be derived for structs"); },
    };

    let expanded = quote! {
        impl #impl_generics Resolve for #name #ty_generics #where_clause {
            type Item = Self;

            fn iter(session: &Session) -> Result<impl Iterator<Item = Self>, ResolveDependencyError> {
                #resolve_body
            }

            fn iter_from_items<__I>(items: __I) -> Result<impl Iterator<Item = Self>, ResolveDependencyError>
            where
                __I: Iterator<Item = Self::Item>,
            {
                Ok(items)
            }
        }
    };

    TokenStream::from(expanded)
}