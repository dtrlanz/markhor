use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields, Type};

struct FieldConfig {
    // Each
    each: bool,
    
    // Filtering
    filter: Option<Expr>,
    filter_map: Option<(Expr, Type)>, // Acts as both Filtering and Mapping

    // Mapping
    map: Option<(Expr, Type)>,
    key: Option<Expr>,

    // Sorting
    sort_by: Option<Expr>,
    sort_by_key: Option<Expr>,
    sort_by_field: Option<Expr>,
}

impl FieldConfig {
    fn needs_custom_iter(&self) -> bool {
        self.filter.is_some()
            || self.filter_map.is_some()
            || self.map.is_some()
            || self.key.is_some()
            || self.sort_by.is_some()
            || self.sort_by_key.is_some()
            || self.sort_by_field.is_some()
    }
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
        "This attribute requires a closure with an explicitly typed argument. \n\
        Example: `#[resolve(map = |x: SourceType| ...)]`"
    ))
}

fn parse_resolve_attrs(field: &syn::Field) -> syn::Result<FieldConfig> {
    let mut config = FieldConfig { 
        each: false, 
        filter: None, 
        filter_map: None, 
        map: None, 
        key: None,
        sort_by: None,
        sort_by_key: None,
        sort_by_field: None,
    };

    let mut filtering_spans = Vec::new();
    let mut mapping_spans = Vec::new();
    let mut sorting_spans = Vec::new();

    for attr in &field.attrs {
        if attr.path().is_ident("resolve") {
            attr.parse_nested_meta(|meta| {
                // Each
                if meta.path.is_ident("each") {
                    config.each = true;
                    Ok(())
                } 
                // Filtering & Mapping
                else if meta.path.is_ident("filter") {
                    config.filter = Some(meta.value()?.parse()?);
                    filtering_spans.push("filter");
                    Ok(())
                } else if meta.path.is_ident("filter_map") {
                    let expr: Expr = meta.value()?.parse()?;
                    let src_ty = extract_source_type_from_closure(&expr)?;
                    config.filter_map = Some((expr, src_ty));
                    filtering_spans.push("filter_map");
                    mapping_spans.push("filter_map"); // Conflicts with map & key too
                    Ok(())
                } else if meta.path.is_ident("map") {
                    let expr: Expr = meta.value()?.parse()?;
                    let src_ty = extract_source_type_from_closure(&expr)?;
                    config.map = Some((expr, src_ty));
                    mapping_spans.push("map");
                    Ok(())
                } else if meta.path.is_ident("key") {
                    config.key = Some(meta.value()?.parse()?);
                    mapping_spans.push("key");
                    Ok(())
                } 
                // Sorting
                else if meta.path.is_ident("sort_by") {
                    config.sort_by = Some(meta.value()?.parse()?);
                    sorting_spans.push("sort_by");
                    Ok(())
                } else if meta.path.is_ident("sort_by_key") {
                    config.sort_by_key = Some(meta.value()?.parse()?);
                    sorting_spans.push("sort_by_key");
                    Ok(())
                } else if meta.path.is_ident("sort_by_field") {
                    config.sort_by_field = Some(meta.value()?.parse()?);
                    sorting_spans.push("sort_by_field");
                    Ok(())
                } 
                else {
                    Err(meta.error("unsupported resolve attribute"))
                }
            })?;
        }
    }

    // Mutually Exclusive Validations
    if filtering_spans.len() > 1 {
        return Err(syn::Error::new_spanned(field, format!("Filtering attributes ({}) are mutually exclusive.", filtering_spans.join(", "))));
    }
    if mapping_spans.len() > 1 {
        return Err(syn::Error::new_spanned(field, format!("Mapping attributes ({}) are mutually exclusive.", mapping_spans.join(", "))));
    }
    if sorting_spans.len() > 1 {
        return Err(syn::Error::new_spanned(field, format!("Sorting attributes ({}) are mutually exclusive.", sorting_spans.join(", "))));
    }

    // Not Yet Implemented Checks
    if config.filter_map.is_some() {
        return Err(syn::Error::new_spanned(field, "The `filter_map` attribute is not yet implemented."));
    }
    if config.sort_by_key.is_some() {
        return Err(syn::Error::new_spanned(field, "The `sort_by_key` attribute is not yet implemented."));
    }
    if config.sort_by_field.is_some() {
        return Err(syn::Error::new_spanned(field, "The `sort_by_field` attribute is not yet implemented."));
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
        Data::Struct(ref data_struct) => match &data_struct.fields {
            Fields::Unit => {
                quote! {
                    let __items = std::iter::once(Self);
                    Ok(__items)
                }
            }
            fields => {
                // Unify logic for classic and tuple structs
                let (fields_iter, is_named) = match fields {
                    Fields::Named(f) => (&f.named, true),
                    Fields::Unnamed(f) => (&f.unnamed, false),
                    Fields::Unit => unreachable!(),
                };

                let mut field_vars = Vec::new();
                let mut field_init_prefixes = Vec::new();
                
                let mut non_each_inits = Vec::new();
                let mut each_loops = Vec::new();
                let mut normal_fields = Vec::new();

                for (i, field) in fields_iter.iter().enumerate() {
                    // Generate variable identifiers (`ident` for named, `__field_0` for tuples)
                    let var_ident = match &field.ident {
                        Some(ident) => ident.clone(),
                        None => quote::format_ident!("__field_{}", i),
                    };

                    // Generate struct init syntax (`ident:` for named, empty for tuples)
                    let init_prefix = match &field.ident {
                        Some(ident) => quote! { #ident: },
                        None => quote! {},
                    };

                    let attrs = match parse_resolve_attrs(field) {
                        Ok(a) => a,
                        Err(e) => return e.to_compile_error().into(),
                    };
                    
                    let ty = &field.ty;
                    field_vars.push(var_ident.clone());
                    field_init_prefixes.push(init_prefix);

                    // 1. Determine base iterator call and any map transformations
                    let (base_iter_call, map_step) = if let Some((map_expr, src_ty)) = &attrs.map {
                        (
                            quote! { <#src_ty as Resolve>::iter(session)? }, 
                            quote! { let __iter = std::iter::Iterator::map(__iter, #map_expr); }
                        )
                    } else if let Some((_filter_map_expr, _src_ty)) = &attrs.filter_map {
                        // TODO: Implement filter_map
                        (quote! {}, quote! {})
                    } else if let Some(key_expr) = &attrs.key {
                        (
                            quote! {
                                {
                                    // Local scoped trait to peel the inner 'Value' type out of the target KV type
                                    trait __ResolveKeyTupleExtractor { type Value; }
                                    impl<__K, __V> __ResolveKeyTupleExtractor for (__K, __V) { type Value = __V; }
                                    < <<#ty as Resolve>::Item as __ResolveKeyTupleExtractor>::Value as Resolve >::iter(session)?
                                }
                            },
                            quote! { 
                                let __iter = {
                                    // Helper function bridges the inference gap by locking the closure's inputs tightly to the iterator's outputs
                                    fn __apply_key_mapper<__I, __V, __K, __F>(
                                        __iter: __I,
                                        mut __key_fn: __F,
                                    ) -> impl std::iter::Iterator<Item = (__K, __V)>
                                    where
                                        __I: std::iter::Iterator<Item = __V>,
                                        __F: std::ops::FnMut(&__V) -> __K,
                                    {
                                        std::iter::Iterator::map(__iter, move |__item| {
                                            let __k = __key_fn(&__item);
                                            (__k, __item)
                                        })
                                    }
                                    __apply_key_mapper(__iter, #key_expr)
                                };
                            }
                        )
                    } else {
                        (
                            quote! { <<#ty as Resolve>::Item as Resolve>::iter(session)? }, 
                            quote! {}
                        )
                    };

                    // 2. Determine filter transformations
                    let filter_step = if let Some(f) = &attrs.filter {
                        quote! { let __iter = std::iter::Iterator::filter(__iter, #f); }
                    } else if let Some((_filter_map_expr, _src_ty)) = &attrs.filter_map {
                        // TODO: Implement filter_map (Filtering portion)
                        quote! {}
                    } else {
                        quote! {}
                    };

                    // 3. Determine eager sorting transformations
                    let sort_step = if let Some(f) = &attrs.sort_by {
                        quote! {
                            let mut __vec = std::iter::Iterator::collect::<std::vec::Vec<_>>(__iter);
                            __vec.sort_by(#f);
                            let __iter = std::iter::IntoIterator::into_iter(__vec);
                        }
                    } else if let Some(_f) = &attrs.sort_by_key {
                        // TODO: Implement sort_by_key
                        quote! {}
                    } else if let Some(_f) = &attrs.sort_by_field {
                        // TODO: Implement sort_by_field
                        quote! {}
                    } else {
                        quote! {}
                    };

                    // 4. Construct the resolved iterator expression
                    let needs_custom_iter = attrs.needs_custom_iter();
                    let iter_expr = if needs_custom_iter {
                        quote! {
                            {
                                let __iter = #base_iter_call;
                                #filter_step
                                #map_step
                                #sort_step
                                <#ty as Resolve>::iter_from_items(__iter)?
                            }
                        }
                    } else {
                        quote! { <#ty as Resolve>::iter(session)? }
                    };

                    if attrs.each {
                        each_loops.push((var_ident.clone(), iter_expr));
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

                        non_each_inits.push(quote! { let #var_ident = #init_tokens; });
                        normal_fields.push(var_ident);
                    }
                }

                // Final Struct construction logic
                if each_loops.is_empty() {
                    let mut struct_inits = Vec::new();
                    for (var_ident, init_prefix) in field_vars.iter().zip(&field_init_prefixes) {
                        struct_inits.push(quote! { #init_prefix #var_ident });
                    }
                    
                    let construct_expr = if is_named {
                        quote! { Self { #( #struct_inits ),* } }
                    } else {
                        quote! { Self ( #( #struct_inits ),* ) }
                    };

                    quote! {
                        #( #non_each_inits )*
                        let __items = std::iter::once(#construct_expr);
                        Ok(__items)
                    }
                } else {
                    let mut each_inits = Vec::new();
                    let mut each_fields = Vec::new();
                    
                    // Setup the initial bindings (evaluating outer iterator once, caching inner loops)
                    for (i, (name, iter_expr)) in each_loops.iter().enumerate() {
                        each_fields.push(name.clone());
                        if i == 0 {
                            each_inits.push(quote! { let #name = #iter_expr; });
                        } else {
                            each_inits.push(quote! { let #name = std::iter::Iterator::collect::<std::vec::Vec<_>>(#iter_expr); });
                        }
                    }
                    
                    let n = each_fields.len() - 1;
                    let innermost_field = &each_fields[n];
                    
                    // Clone all fields inside instantiation except the innermost each field
                    let mut struct_inits = Vec::new();
                    for (var_ident, init_prefix) in field_vars.iter().zip(&field_init_prefixes) {
                        if var_ident == innermost_field {
                            struct_inits.push(quote! { #init_prefix #var_ident });
                        } else {
                            struct_inits.push(quote! { #init_prefix #var_ident.clone() });
                        }
                    }
                    
                    let mut current_expr = if is_named {
                        quote! { Self { #( #struct_inits ),* } }
                    } else {
                        quote! { Self ( #( #struct_inits ),* ) }
                    };
                    
                    // Fold iterators backwards from innermost to outermost
                    for i in (0..=n).rev() {
                        let field_i = &each_fields[i];
                        
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
                                let f = &each_fields[j];
                                clones.push(quote! { let #f = #f.clone(); });
                            }
                            // Vectors for deeper loops
                            if i + 2 <= n {
                                for j in (i + 2)..=n {
                                    let f = &each_fields[j];
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