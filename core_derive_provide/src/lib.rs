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
        Example: `#[provide(map = |x: SourceType| ...)]`"
    ))
}

/// Helper to parse the top-level `#[provide(crate = "...")]` helper attribute on structs.
fn parse_struct_crate_path(attrs: &[syn::Attribute]) -> syn::Result<Option<syn::Path>> {
    let mut crate_path = None;
    for attr in attrs {
        if attr.path().is_ident("provide") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("crate") {
                    let path_str: syn::LitStr = meta.value()?.parse()?;
                    crate_path = Some(path_str.parse()?);
                    Ok(())
                } else {
                    Err(meta.error("unsupported struct-level provide attribute"))
                }
            })?;
        }
    }
    Ok(crate_path)
}

fn parse_provide_attrs(field: &syn::Field) -> syn::Result<FieldConfig> {
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
        if attr.path().is_ident("provide") {
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
                    Err(meta.error("unsupported provide attribute"))
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

/// Scans the struct's generics to find a namespace prefix (e.g. `___`) that is
/// guaranteed not to collide with any user-defined generic names.
fn find_hygiene_prefix(generics: &syn::Generics) -> String {
    let mut prefix = "___".to_string();
    while generics.params.iter().any(|param| {
        let ident_str = match param {
            syn::GenericParam::Type(t) => t.ident.to_string(),
            syn::GenericParam::Lifetime(l) => l.lifetime.ident.to_string(),
            syn::GenericParam::Const(c) => c.ident.to_string(),
        };
        ident_str.starts_with(&prefix)
    }) {
        prefix.push('_');
    }
    prefix
}

/// Derives the `Provide` trait for a struct.
///
/// This macro automatically implements dependency resolution for named structs, 
/// tuple structs, and unit structs. By default, it eagerly provides exactly 
/// one instance of each field using `<FieldType as Provide>::first(session)`.
///
/// You can customize how fields are iterated, filtered, mapped, and sorted 
/// using the `#[provide(...)]` attribute.
///
/// # Attribute Groups
/// 
/// Attributes are categorized into groups. You may combine attributes from 
/// *different* groups on a single field, but combining multiple attributes 
/// from the *same* group will result in a compile error.
///
/// ## 1. Iteration (`each`)
/// Normally, a field evaluates to a single instance. Marking a field with 
/// `#[provide(each)]` transforms it into a loop, yielding every available 
/// instance of that dependency.
/// 
/// - **Cartesian Products:** If multiple fields are marked with `each`, the 
///   macro generates a lazy Cartesian product.
/// - **Loop Order:** The order of fields in the struct defines the loop nesting. 
///   The first `each` field forms the outermost loop, and the last `each` field 
///   forms the innermost loop.
///
/// ## 2. Filtering
/// Filters the iterator before yielding items.
/// - `filter = |item| ...`: Takes a closure returning a `bool`. Only items 
///   returning `true` are kept.
//X - `filter_map = |item: SourceType| ...` (Not yet implemented)
///
/// ## 3. Mapping
/// Transforms the dependency type into a different type.
/// - `map = |item: SourceType| ...`: Maps the provided item to a new value.
///   **Note:** Due to Rust's closure type inference limitations inside macros, 
///   you must explicitly annotate the closure's input type.
/// - `key = |item| ...`: A shorthand for mapping an item into a `(Key, Value)` 
///   tuple (useful for `HashMap`). The closure takes a reference (`&item`) and 
///   returns the key.
///
/// ## 4. Sorting
/// Sorts the items before they are yielded. **Note:** Sorting forces eager 
/// evaluation (collecting all items into a `Vec` internally).
/// - `sort_by = |a, b| ...`: Takes a closure returning `std::cmp::Ordering`.
//X - `sort_by_key = |item| ...` (Not yet implemented)
//X - `sort_by_field = field_name` (Not yet implemented)
///
/// # Evaluation Order
/// If multiple attributes are applied to the same field, they are evaluated in 
/// this order:
/// 1. `filter` (operates on the original dependency type)
/// 2. `map` / `key` (transforms the type)
/// 3. `sort_by` (operates on the *mapped* output type)
///
/// # Examples
/// 
/// TODO: add nice examples
#[proc_macro_derive(Provide, attributes(provide))]
pub fn derive_provide(input: TokenStream) -> TokenStream {
    // Parse input tokens into a syntax tree
    let input = parse_macro_input!(input as DeriveInput);
    let name = &input.ident;
    
    // `Provide` crate path (Defaults to `::markhor_core`)
    let crate_path = match parse_struct_crate_path(&input.attrs) {
        Ok(Some(path)) => path,
        Ok(None) => syn::parse_quote! { ::markhor_core },
        Err(e) => return e.to_compile_error().into(),
    };

    // Generate collision-free generic type names for internal helper functions
    let prefix = find_hygiene_prefix(&input.generics);
    let g_i = quote::format_ident!("{}I", prefix);
    let g_v = quote::format_ident!("{}V", prefix);
    let g_k = quote::format_ident!("{}K", prefix);
    let g_f = quote::format_ident!("{}F", prefix);
    let g_dst = quote::format_ident!("{}Dst", prefix);

    // Support generics
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    // Generate the body of the `iter` function based on the struct's fields
    let provide_body = match input.data {
        Data::Struct(ref data_struct) => match &data_struct.fields {
            Fields::Unit => {
                quote! {
                    let __items = ::std::iter::once(Self);
                    ::std::result::Result::Ok(__items)
                }
            }
            fields => {
                // Unified logic for classic and tuple structs
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

                    let attrs = match parse_provide_attrs(field) {
                        Ok(a) => a,
                        Err(e) => return e.to_compile_error().into(),
                    };
                    
                    let ty = &field.ty;
                    field_vars.push(var_ident.clone());
                    field_init_prefixes.push(init_prefix);

                    // 1. Determine base iterator call and any map transformations
                    let (base_iter_call, map_step) = if let Some((map_expr, src_ty)) = &attrs.map {
                        (
                            quote! { <#src_ty as #crate_path::dependencies::Provide>::iter(__session)? }, 
                            quote! { let __iter = ::std::iter::Iterator::map(__iter, #map_expr); }
                        )
                    } else if let Some((_filter_map_expr, _src_ty)) = &attrs.filter_map {
                        // TODO: Implement filter_map
                        (quote! {}, quote! {})
                    } else if let Some(key_expr) = &attrs.key {
                        (
                            quote! {
                                {
                                    // Local scoped trait to peel the inner 'Value' type out of the target KV type
                                    trait __ProvideKeyTupleExtractor { type Value; }
                                    impl<__K, __V> __ProvideKeyTupleExtractor for (__K, __V) { type Value = __V; }
                                    < <<#ty as #crate_path::dependencies::Provide>::Item as __ProvideKeyTupleExtractor>::Value as #crate_path::dependencies::Provide >::iter(__session)?
                                }
                            },
                            quote! { 
                                let __iter = {
                                    // Helper function bridges the inference gap by locking the closure's inputs tightly to the iterator's outputs
                                    fn __apply_key_mapper<#g_i, #g_v, #g_k, #g_f>(
                                        __iter: #g_i,
                                        mut __key_fn: #g_f,
                                    ) -> impl ::std::iter::Iterator<Item = (#g_k, #g_v)>
                                    where
                                        #g_i: ::std::iter::Iterator<Item = #g_v>,
                                        #g_f: ::std::ops::FnMut(&#g_v) -> #g_k,
                                    {
                                        ::std::iter::Iterator::map(__iter, move |__item| {
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
                            quote! { <<#ty as #crate_path::dependencies::Provide>::Item as #crate_path::dependencies::Provide>::iter(__session)? }, 
                            quote! {}
                        )
                    };

                    // 2. Determine filter transformations
                    let filter_step = if let Some(f) = &attrs.filter {
                        quote! { let __iter = ::std::iter::Iterator::filter(__iter, #f); }
                    } else if let Some((_filter_map_expr, _src_ty)) = &attrs.filter_map {
                        // TODO: Implement filter_map
                        quote! {}
                    } else {
                        quote! {}
                    };

                    // 3. Determine eager sorting transformations
                    let sort_step = if let Some(f) = &attrs.sort_by {
                        quote! {
                            let mut __vec = ::std::iter::Iterator::collect::<::std::vec::Vec<_>>(__iter);
                            __vec.sort_by(#f);
                            let __iter = ::std::iter::IntoIterator::into_iter(__vec);
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

                    // 4. Construct the iterator expression
                    let needs_custom_iter = attrs.needs_custom_iter();
                    let iter_expr = if needs_custom_iter {
                        quote! {
                            {
                                let __iter = #base_iter_call;
                                #filter_step
                                #map_step
                                #sort_step
                                <#ty as #crate_path::dependencies::Provide>::iter_from_items(__iter)?
                            }
                        }
                    } else {
                        quote! { <#ty as #crate_path::dependencies::Provide>::iter(__session)? }
                    };

                    if attrs.each {
                        each_loops.push((var_ident.clone(), iter_expr));
                    } else {
                        // Standard field logic
                        let init_tokens = if needs_custom_iter {
                            quote! {
                                {
                                    let mut __field_iter = #iter_expr;
                                    ::std::iter::Iterator::next(&mut __field_iter)
                                        .ok_or_else(|| #crate_path::dependencies::ResolveDependencyError::DependencyNotAvailable(
                                            ::std::any::type_name::<#ty>().to_string()
                                        ))?
                                }
                            }
                        } else {
                            quote! { <#ty as #crate_path::dependencies::Provide>::first(__session)? }
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
                        let __items = ::std::iter::once(#construct_expr);
                        ::std::result::Result::Ok(__items)
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
                            each_inits.push(quote! { let #name = ::std::iter::Iterator::collect::<::std::vec::Vec<_>>(#iter_expr); });
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
                            quote! { ::std::iter::IntoIterator::into_iter(#field_i.clone()) }
                        };
                        
                        if i == n {
                            // Innermost each loop is just a map
                            current_expr = quote! {
                                ::std::iter::Iterator::map(#iter_i, move |#field_i| {
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
                                ::std::iter::Iterator::flat_map(#iter_i, move |#field_i| {
                                    #( #clones )*
                                    #current_expr
                                })
                            };
                        }
                    }
                    
                    quote! {
                        #( #non_each_inits )*
                        #( #each_inits )*
                        ::std::result::Result::Ok(#current_expr)
                    }
                }
            }
        },
        _ => quote! { compile_error!("Provide can only be derived for structs"); },
    };

    let expanded = quote! {
        impl #impl_generics #crate_path::dependencies::Provide for #name #ty_generics #where_clause {
            type Item = Self;

            // Parameter named `__session` to avoid collisions with struct fields named `session`
            fn iter(__session: &#crate_path::dependencies::Session) -> ::std::result::Result<impl ::std::iter::Iterator<Item = Self>, #crate_path::dependencies::ResolveDependencyError> {
                #provide_body
            }

            fn iter_from_items<#g_i>(items: #g_i) -> ::std::result::Result<impl ::std::iter::Iterator<Item = Self>, #crate_path::dependencies::ResolveDependencyError>
            where
                #g_i: ::std::iter::Iterator<Item = Self::Item>,
            {
                ::std::result::Result::Ok(items)
            }
        }
    };

    TokenStream::from(expanded)
}