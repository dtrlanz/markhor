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
                let mut normal_fields = Vec::new();

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

                    // 2. Determine filter transformations
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
                        normal_fields.push(field_name);
                    }
                }

                if each_loops.is_empty() {
                    let mut struct_inits = Vec::new();
                    for field_name in &field_names {
                        struct_inits.push(quote! { #field_name });
                    }
                    
                    quote! {
                        #( #non_each_inits )*
                        let __items = std::iter::once(Self {
                            #( #struct_inits ),*
                        });
                        
                        Ok(__items)
                    }
                } else {
                    let mut each_inits = Vec::new();
                    let mut each_fields = Vec::new();
                    
                    // Setup the initial bindings (evaluating outer iterator once, caching inner loops)
                    for (i, (name, iter_expr)) in each_loops.iter().enumerate() {
                        each_fields.push(*name);
                        if i == 0 {
                            each_inits.push(quote! { let #name = #iter_expr; });
                        } else {
                            each_inits.push(quote! { let #name = std::iter::Iterator::collect::<std::vec::Vec<_>>(#iter_expr); });
                        }
                    }
                    
                    let n = each_fields.len() - 1;
                    let innermost_field = each_fields[n];
                    
                    // Clone all fields inside instantiation except the innermost each field
                    let mut struct_inits = Vec::new();
                    for field_name in &field_names {
                        if *field_name == innermost_field {
                            struct_inits.push(quote! { #field_name });
                        } else {
                            struct_inits.push(quote! { #field_name: #field_name.clone() });
                        }
                    }
                    
                    let mut current_expr = quote! {
                        Self { #( #struct_inits ),* }
                    };
                    
                    // Fold iterators backwards from innermost to outermost
                    for i in (0..=n).rev() {
                        let field_i = each_fields[i];
                        
                        let iter_i = if i == 0 {
                            quote! { #field_i }
                        } else {
                            quote! { std::iter::IntoIterator::into_iter(#field_i.clone()) }
                        };
                        
                        if i == n {
                            // Innermost each loop is just a map
                            current_expr = quote! {
                                std::iter::Iterator::map(#iter_i, move |#field_i| {
                                    #current_expr
                                })
                            };
                        } else {
                            // Outer each loops use flat_map and prepare clones for the FnMut bounds of deeper iterators
                            let mut clones = Vec::new();
                            for f in &normal_fields {
                                clones.push(quote! { let #f = #f.clone(); });
                            }
                            // Variables from preceding loops (already expanded)
                            for j in 0..=i {
                                let f = each_fields[j];
                                clones.push(quote! { let #f = #f.clone(); });
                            }
                            // Vectors for deeper loops
                            if i + 2 <= n {
                                for j in (i + 2)..=n {
                                    let f = each_fields[j];
                                    clones.push(quote! { let #f = #f.clone(); });
                                }
                            }
                            
                            current_expr = quote! {
                                std::iter::Iterator::flat_map(#iter_i, move |#field_i| {
                                    #( #clones )*
                                    #current_expr
                                })
                            };
                        }
                    }
                    
                    quote! {
                        #( #non_each_inits )*
                        #( #each_inits )*
                        Ok(#current_expr)
                    }
                }
            }
            // Tuple structs: struct Foo(Bar, Baz)
            Fields::Unnamed(ref _fields) => {
                quote! { compile_error!("Currently only named structs are supported in this POC."); }
            }
            Fields::Unit => {
                quote! {
                    let __items = std::iter::once(Self);
                    Ok(__items)
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