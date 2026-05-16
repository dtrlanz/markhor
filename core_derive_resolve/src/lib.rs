use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, Data, DeriveInput, Expr, Fields};

struct FieldConfig {
    filter: Option<Expr>,
    each: bool,
}

fn parse_resolve_attrs(attrs: &[syn::Attribute]) -> syn::Result<FieldConfig> {
    let mut config = FieldConfig { filter: None, each: false };
    for attr in attrs {
        if attr.path().is_ident("resolve") {
            attr.parse_nested_meta(|meta| {
                if meta.path.is_ident("filter") {
                    config.filter = Some(meta.value()?.parse()?);
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
        Data::Struct(ref data_struct) => match data_struct.fields {
            // Structs with named fields: struct Foo { bar: Bar }
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

                    if attrs.each {
                        let iter_expr = match attrs.filter {
                            None => quote! { <#ty as Resolve>::iter(session)? },
                            Some(f) => quote! { 
                                {
                                    // Extract the inner iter, apply the filter, and recombine through iter_from_items
                                    let __base_iter = <<#ty as Resolve>::Item as Resolve>::iter(session)?;
                                    let __filtered = std::iter::Iterator::filter(__base_iter, #f);
                                    <#ty as Resolve>::iter_from_items(__filtered)?
                                }
                            },
                        };
                        
                        each_loops.push((field_name, iter_expr));
                    } else {
                        // Standard field logic perfectly abstracted by the trait
                        let init_tokens = match attrs.filter {
                            None => quote! { <#ty as Resolve>::first(session)? },
                            Some(f) => quote! {
                                {
                                    let __base_iter = <<#ty as Resolve>::Item as Resolve>::iter(session)?;
                                    let __filtered = std::iter::Iterator::filter(__base_iter, #f);
                                    let mut __field_iter = <#ty as Resolve>::iter_from_items(__filtered)?;
                                    
                                    std::iter::Iterator::next(&mut __field_iter)
                                        .ok_or_else(|| ResolveDependencyError::DependencyNotAvailable(
                                            std::any::type_name::<#ty>().to_string()
                                        ))?
                                }
                            },
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
                        
                        // We now use the clone-aware initializers
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